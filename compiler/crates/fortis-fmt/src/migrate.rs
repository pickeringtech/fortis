//! The v0.0.2 reading migrations (register X-345): rewrites from a v0.0.1 form to its v0.0.2 form, each applied only
//! where it keeps the program's meaning. `fortis fmt` applies them before laying the module out; `--no-migrate`
//! leaves the tokens as written.
//!
//! - **The entry `main` (4.6, R4 and R8).** In the entry module, `export` is dropped from `main`. When `main`'s body
//!   creates its capabilities with `let caps = Host.capabilities()` and reaches them only as `caps.console` and its
//!   siblings, or through a `let Capabilities(console: out, …) = move caps` destructuring, the creation and the
//!   destructuring go, each capability the body uses becomes a parameter of its type (named after the field, or after
//!   the destructuring's binding), and the import names those types instead of `Host`. `args: List[String]` is
//!   dropped when nothing reads it. Any other use of the capabilities record, a name the new parameters would collide
//!   with, or a use inside an interpolation leaves `main` as it is.
//! - **Tests (14, R7).** In `test` and `property` bodies, `or fail` after a call or after integer arithmetic is
//!   implied: `f() or fail` becomes `f()`, `a * b or fail` becomes `a * b`, and `(f() or fail)` loses its parentheses
//!   too where they held only the handler: `(a + b or fail) * c` becomes `(a + b) * c`, never `a + b * c` (D-324). A
//!   lambda's body is its own function and is left alone.
//! - **Map lookups (6.7, R11).** `m.get(copy key)` and `m.get(String.from(view))` become `m.get(key)` and
//!   `m.get(view)`: a `Map[String, V]` reads its key through a view. Only where `m` is the prelude's `Map` with
//!   `String` keys, and `String` the prelude's, as the checker resolves them: a spelling is not a resolution (D-329).
//!
//! The tests' and the lookups' rewrites need the checker (a function returning an `Outcome` it built is not a failing
//! call; `get` of another class may want an owned key). The caller's `check` gives what it establishes about a text
//! (`Facts`), on its own or in its package, or None when it rejects it. A rewrite is kept only when the module
//! checks before and after it and, beyond that, means the same: its tree is the original's with only the handler or
//! the key copy taken out (`shape`), each call that lost its `or fail` is one the checker gives the implied `or fail`,
//! each region that lost one is a checked region with it, and each lookup is still a `Map[String, V]` lookup.

use std::collections::{BTreeMap, BTreeSet};

use fortis_names::reading::{call_name_token, capability_field, CAPABILITY_TYPES};
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{declarations, imports, C, N};

/// A replacement of the text between two (line, column) positions, 1-based, in scalars; `end` is exclusive.
#[derive(Clone, Debug)]
struct Edit {
    start: (usize, usize),
    end: (usize, usize),
    text: String,
}

fn tok_end(t: &Token) -> (usize, usize) {
    match t.text.rfind('\n') {
        Some(i) => (t.line + t.text.matches('\n').count(), t.text[i + 1..].chars().count() + 1),
        None => (t.line, t.col + t.text.chars().count()),
    }
}

fn apply(src: &str, edits: &[Edit]) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut starts = vec![0usize];
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            starts.push(i + 1);
        }
    }
    let off = |(l, c): (usize, usize)| -> usize { starts.get(l.saturating_sub(1)).map_or(chars.len(), |s| (s + c.saturating_sub(1)).min(chars.len())) };
    let mut spans: Vec<(usize, usize, &str)> = edits.iter().map(|e| (off(e.start), off(e.end), e.text.as_str())).collect();
    spans.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = chars;
    let mut last = usize::MAX;
    for (s, e, t) in spans {
        if e > last {
            continue; // overlapping edits: the later one wins, the earlier is dropped (never both)
        }
        out.splice(s..e, t.chars());
        last = s;
    }
    out.into_iter().collect()
}

/// The whole line `l` (with its newline), when nothing else shares it with the tokens from `first` to `last`.
fn line_of_tokens(all: &[Token], first: &Token, last: &Token) -> Option<Edit> {
    let others = all.iter().any(|t| {
        !matches!(t.kind, Kind::Newline | Kind::Nl | Kind::Eof) && t.line >= first.line && t.line <= tok_end(last).0 && ((t.line, t.col) < (first.line, first.col) || (t.line, t.col) > (last.line, last.col))
    });
    if others {
        return None;
    }
    Some(Edit { start: (first.line, 1), end: (tok_end(last).0 + 1, 1), text: String::new() })
}

/// What the checker establishes about a module's text that the typed rewrites read: the module is accepted, and these
/// hold of it (positions are (line, column) of tokens).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// the name tokens of the test bodies' calls with an implied `or fail` (14, LANG-14-03)
    pub implied_fails: BTreeSet<(usize, usize)>,
    /// the first operator of each checked region of a test body with an implied `or fail` (14, 6.4)
    pub implied_regions: BTreeSet<(usize, usize)>,
    /// the member names of the `get` calls whose receiver is the prelude's `Map` with the prelude's `String` keys
    pub string_map_gets: BTreeSet<(usize, usize)>,
    /// `String` names the prelude's `String` in the module, not a declaration or an import of its own
    pub prelude_string: bool,
}

impl Facts {
    /// The facts of the module `root`, from the typer of its final pass, run with `record_types` set.
    pub fn of(ty: &fortis_types::typer::Typer, root: N) -> Facts {
        use fortis_types::types::{strip, Type};
        let standard = |name: &str| ty.dc.table.get(name).map_or(false, |d| fortis_types::decls::is_standard(&d.module));
        let core = |t: &Type, name: &str| matches!(strip(t), Type::Named(n) if n.name == name && (n.package.is_empty() || fortis_types::decls::is_standard(&n.package)));
        let mut string_map_gets = BTreeSet::new();
        if let Some(types) = &ty.node_types {
            if standard("Map") && standard("String") {
                for pe in root.find_all("postfixExpr") {
                    let kids: Vec<N> = pe.kids().into_iter().filter_map(|k| k.node()).collect();
                    for j in 1..kids.len() {
                        let pf = kids[j];
                        let mc = if pf.name() == "methodCall" { Some(pf) } else { pf.kid(0).and_then(|k| k.node()).filter(|k| k.name() == "methodCall") };
                        let Some(get) = mc.and_then(|m| m.child_node("memberName")).and_then(|m| m.first_token()).filter(|t| t.text == "get") else { continue };
                        // the receiver is the value before this postfix: the primary's, or the previous postfix's
                        let receiver = types.get(&kids[j - 1].id());
                        let string_map = receiver.map_or(false, |t| match strip(t) {
                            Type::Named(n) => core(t, "Map") && n.args.len() == 2 && core(&n.args[0], "String"),
                            _ => false,
                        });
                        if string_map {
                            string_map_gets.insert((get.line, get.col));
                        }
                    }
                }
            }
        }
        Facts { implied_fails: ty.implied_fails.clone(), implied_regions: ty.implied_regions.clone(), string_map_gets, prelude_string: standard("String") }
    }
}

/// The `check` of a module outside any package: its facts when its text checks with no error on its own.
pub fn facts_alone(file: &str) -> impl Fn(&str) -> Option<Facts> + '_ {
    move |text: &str| {
        let p = fortis_syntax::parse_module(text).ok()?;
        let (records, facts) = fortis_types::check::check_parsed_typed(&p, file, |ty, root| Facts::of(ty, root));
        (!records.iter().any(|r| r.severity == "error")).then_some(facts)
    }
}

/// The `check` of the module at `file` in the package whose directory is `dir`: its facts when, with its text, every
/// module of the package set checks with no error (the file replaced in memory; nothing is written).
pub fn facts_in_package<'p>(dir: &'p std::path::Path, file: &'p std::path::Path) -> impl Fn(&str) -> Option<Facts> + 'p {
    move |text: &str| {
        let search = dir.parent().unwrap_or(std::path::Path::new("."));
        let mut ps = fortis_names::resolve::PackageSet::load(dir, search).ok()?;
        let parsed = std::sync::Arc::new(fortis_syntax::parse_module(text).ok()?);
        let target = std::fs::canonicalize(file).ok()?;
        let mut key = None;
        for (_, pkg) in ps.packages.iter_mut() {
            for (mpath, m) in pkg.modules.iter_mut() {
                if std::fs::canonicalize(&m.file).ok().as_ref() == Some(&target) {
                    m.parsed = Some(parsed.clone());
                    m.error = None;
                    key = Some(format!("{}.{}", pkg.name, mpath));
                }
            }
        }
        let key = key?;
        if ps.run().iter().any(|x| x.severity == "error") {
            return None;
        }
        let (records, built) = fortis_types::check::check_package_set_typed(&ps, |ty, root, k, _| (k == key).then(|| Facts::of(ty, root)));
        if records.iter().any(|x| x.severity == "error") {
            return None;
        }
        built.into_iter().find_map(|(_, f)| f)
    }
}

/// The migrated source. `entry`: the module is a program's entry module (a single file, or the manifest's `entry` of
/// a package), so its `main` is the entry `main`. `check(text)`: the facts of the module with that text where it
/// lives, alone or in its package, or None when it does not check; each rewrite is kept only when the module checks
/// before and after it, and the typed ones only when they mean the same (the module documentation).
pub fn migrate(src: &str, entry: bool, check: &dyn Fn(&str) -> Option<Facts>) -> String {
    let Ok(parsed) = fortis_syntax::parse_module(src) else { return src.to_string() };
    let root = N::new(&parsed.root, &parsed.toks);
    let main_found = if entry { entry_main(root, &parsed.toks).unwrap_or_default() } else { vec![] };
    let removals = tests(root);
    let lookups = map_gets(root);
    if main_found.is_empty() && removals.is_empty() && lookups.is_empty() {
        return src.to_string();
    }
    // every rewrite needs a module that checks: in a rejected module it could change what is reported, or turn the
    // rejection into an acceptance (a module whose own `List` makes `args: List[String]` the wrong form of 4.6 would
    // lose that `args`)
    let Some(before) = check(src) else { return src.to_string() };
    let mut kept: Vec<Edit> = Vec::new();
    // the entry rewrite is syntactic, and kept when the module still checks after it
    if !main_found.is_empty() && check(&apply(src, &main_found)).is_some() {
        kept = main_found;
    }
    // the tests' handlers: the parentheses around one go only where the grouping stays what it was
    if !removals.is_empty() {
        let subst: BTreeMap<usize, Repl> = removals.iter().map(|r| (r.outcome, Repl::Node(r.left))).collect();
        let want = shape(root, &subst);
        let handlers: Vec<Edit> = removals.iter().map(|r| r.handler.clone()).collect();
        let mut edits = handlers.clone();
        for r in &removals {
            if let Some(parens) = &r.parens {
                let mut trial = handlers.clone();
                trial.extend(parens.iter().cloned());
                if same_shape(&apply(src, &trial), &want) {
                    edits.extend(parens.iter().cloned());
                }
            }
        }
        if !same_shape(&apply(src, &edits), &want) {
            edits = handlers;
        }
        let mut trial = kept.clone();
        trial.extend(edits.iter().cloned());
        let text = apply(src, &trial);
        if same_shape(&apply(src, &edits), &want) {
            if let (Some(after), Ok(again)) = (check(&text), fortis_syntax::parse_module(&text)) {
                let again_root = N::new(&again.root, &again.toks);
                let implied = removals.iter().all(|r| match r.implied {
                    Implied::Call(at) => after.implied_fails.contains(&moved(at, &trial)),
                    Implied::Region(at) => region_holds(again_root, moved(at, &trial), &after.implied_regions),
                });
                if implied {
                    kept = trial;
                }
            }
        }
    }
    // the lookups, where the checker resolved the receiver (and `String`) to the prelude's
    let lookups: Vec<Lookup> = lookups.into_iter().filter(|l| before.string_map_gets.contains(&l.get) && (!l.from || before.prelude_string)).collect();
    if !lookups.is_empty() {
        let subst: BTreeMap<usize, Repl> = lookups.iter().map(|l| (l.arg, Repl::Text(l.key.clone()))).collect();
        let edits: Vec<Edit> = lookups.iter().map(|l| l.edit.clone()).collect();
        let mut trial = kept.clone();
        trial.extend(edits.iter().cloned());
        let text = apply(src, &trial);
        if same_shape(&apply(src, &edits), &shape(root, &subst)) {
            if let Some(after) = check(&text) {
                if lookups.iter().all(|l| after.string_map_gets.contains(&moved(l.get, &trial))) {
                    kept = trial;
                }
            }
        }
    }
    if kept.is_empty() {
        src.to_string()
    } else {
        apply(src, &kept)
    }
}

/// A position of the original text, outside every edit, in the text the edits make (none inserts a line break).
fn moved(p: (usize, usize), edits: &[Edit]) -> (usize, usize) {
    let mut before: Vec<&Edit> = edits.iter().filter(|e| e.end <= p).collect();
    before.sort_by(|a, b| b.start.cmp(&a.start));
    let mut p = p;
    for e in before {
        if p.0 == e.end.0 {
            p = (e.start.0, e.start.1 + e.text.chars().count() + (p.1 - e.end.1));
        } else {
            p = (p.0 - (e.end.0 - e.start.0), p.1);
        }
    }
    p
}

/// What a rewrite leaves in place of a node: another node of the tree (a handler's left operand), or the shape of
/// a name (a lookup's key).
enum Repl<'a> {
    Node(N<'a>),
    Text(String),
}

/// The grouping of a tree: rules with one child and the parentheses of `( expr )` are transparent, so two texts
/// with one shape have the same tokens grouped alike; `subst` gives a node's shape in place of its own (the shape of
/// what a rewrite leaves there).
fn shape(n: N, subst: &BTreeMap<usize, Repl>) -> String {
    fn walk(n: N, subst: &BTreeMap<usize, Repl>, out: &mut String) {
        match subst.get(&n.id()) {
            Some(Repl::Node(m)) => return walk(*m, subst, out),
            Some(Repl::Text(s)) => {
                out.push_str(s);
                return;
            }
            None => {}
        }
        let kids: Vec<C> = n.kids().into_iter().filter(|k| k.tok().map_or(true, |t| !matches!(t.kind, Kind::Newline | Kind::Nl | Kind::Eof))).collect();
        if n.name() == "primaryExpr" && kids.len() == 3 && kids[0].text_is("(") && kids[2].text_is(")") {
            if let Some(inner) = kids[1].node() {
                return walk(inner, subst, out);
            }
        }
        if kids.len() == 1 {
            match kids[0] {
                C::N(m) => walk(m, subst, out),
                C::T(t) => {
                    out.push_str(&t.text);
                    out.push(' ');
                }
            }
            return;
        }
        out.push('[');
        for k in kids {
            match k {
                C::N(m) => walk(m, subst, out),
                C::T(t) => {
                    out.push_str(&t.text);
                    out.push(' ');
                }
            }
        }
        out.push(']');
    }
    let mut out = String::new();
    walk(n, subst, &mut out);
    out
}

/// Whether `text` parses to the shape `want`.
fn same_shape(text: &str, want: &str) -> bool {
    fortis_syntax::parse_module(text).map_or(false, |p| shape(N::new(&p.root, &p.toks), &BTreeMap::new()) == want)
}

/// Whether the operator at `op` belongs to a checked region the checker gives an implied `or fail` (its first
/// operator is among `regions`).
fn region_holds(root: N, op: (usize, usize), regions: &BTreeSet<(usize, usize)>) -> bool {
    root.walk().into_iter().any(|n| {
        let ops = fortis_types::typer::expr::region_operators(C::N(n));
        ops.contains(&op) && ops.iter().next().map_or(false, |first| regions.contains(first))
    })
}

/// A lookup R11 may rewrite: the edit, the `get` member name, the argument node, the key it leaves, and whether
/// the key was `String.from(view)` (else `copy key`).
struct Lookup {
    edit: Edit,
    get: (usize, usize),
    arg: usize,
    key: String,
    from: bool,
}

/// The lookups' candidates (R11): `.get(copy key)` and `.get(String.from(view))` with a plain name as the key.
fn map_gets(root: N) -> Vec<Lookup> {
    let mut out = Vec::new();
    for mc in root.find_all("methodCall") {
        let Some(mn) = mc.child_node("memberName") else { continue };
        let Some(get) = mn.first_token().filter(|t| t.text == "get") else { continue };
        let Some(al) = mc.child_node("argList") else { continue };
        let args = al.child_nodes("arg");
        if args.len() != 1 || !args[0].direct_tokens().is_empty() {
            continue;
        }
        let ts = args[0].tokens();
        let words: Vec<&str> = ts.iter().map(|t| t.text.as_str()).collect();
        let at = (get.line, get.col);
        if ts.len() == 2 && words[0] == "copy" && ts[1].kind == Kind::Lower {
            let edit = Edit { start: (ts[0].line, ts[0].col), end: (ts[1].line, ts[1].col), text: String::new() };
            out.push(Lookup { edit, get: at, arg: args[0].id(), key: format!("{} ", ts[1].text), from: false });
        } else if ts.len() == 6 && words[..4] == ["String", ".", "from", "("] && ts[4].kind == Kind::Lower && words[5] == ")" {
            let edit = Edit { start: (ts[0].line, ts[0].col), end: tok_end(ts[5]), text: ts[4].text.clone() };
            out.push(Lookup { edit, get: at, arg: args[0].id(), key: format!("{} ", ts[4].text), from: true });
        }
    }
    out
}

fn words(n: N) -> String {
    n.tokens().iter().map(|t| t.text.as_str()).collect::<Vec<_>>().concat()
}

/// The capabilities record's field that holds `ty`, and the type a field holds.
fn field_type(field: &str) -> Option<&'static str> {
    CAPABILITY_TYPES.iter().copied().find(|t| capability_field(t) == field)
}

/// The edits of the entry `main` (R4, R8), or None when `main` is not rewritten at all.
fn entry_main(root: N, toks: &[Token]) -> Option<Vec<Edit>> {
    let decls = declarations(root);
    let main = decls.iter().find(|d| d.kind == "func" && d.name.as_deref() == Some("main"))?;
    let head = main.node.find("funcHead")?;
    let sig = head.find("funcSignature")?;
    let body = main.node.find("funcBody")?.child_node("block")?;
    let mut edits = Vec::new();
    // `export` is implied on the entry main (R8)
    let decl_node = root.kids().into_iter().filter_map(|c| c.node()).find(|d| d.name() == "declaration" && d.find("funcDecl").map_or(false, |f| f.same(main.node)))?;
    if let Some(ex) = decl_node.direct_tokens().into_iter().find(|t| t.text == "export") {
        let next = decl_node.tokens().into_iter().find(|t| (t.line, t.col) > (ex.line, ex.col))?;
        edits.push(Edit { start: (ex.line, ex.col), end: (next.line, next.col), text: String::new() });
    }
    // the parameter list: v0.0.1's `args: List[String]`, or none; anything else is already v0.0.2 and stays
    let pl = sig.find("paramList");
    let params: Vec<N> = pl.map(|p| p.child_nodes("param")).unwrap_or_default();
    let old_form = params.is_empty() || (params.len() == 1 && words(params[0]) == "args:List[String]");
    if !old_form {
        return Some(edits);
    }
    let body_toks = body.tokens();
    let lower_uses = |name: &str| -> usize {
        let n = body_toks.iter().filter(|t| t.kind == Kind::Lower && t.text == name).count();
        let in_holes = body_toks.iter().filter(|t| t.kind == Kind::Str).flat_map(|t| t.interps.iter()).filter(|(s, _, _)| mentions(s, name)).count();
        n + in_holes
    };
    let args_used = !params.is_empty() && lower_uses("args") > 0;
    // the capabilities: `let caps = Host.capabilities()` among the body's own statements
    let host_import = imports(root).into_iter().find(|i| i.path.join(".") == "fortis.host" && i.members.as_ref().map_or(false, |m| m.iter().any(|x| x == "Host")));
    let caps = caps_rewrite(root, body, toks, &body_toks, host_import.is_some());
    let (cap_params, cap_edits) = match caps {
        Some(x) => x,
        None => (vec![], vec![]),
    };
    if cap_params.is_empty() && cap_edits.is_empty() {
        // no capability rewrite: only `args` may go
        if !params.is_empty() && !args_used {
            if let (Some(open), Some(close)) = (sig.direct_tokens().into_iter().find(|t| t.text == "("), sig.direct_tokens().into_iter().find(|t| t.text == ")")) {
                edits.push(Edit { start: tok_end(open), end: (close.line, close.col), text: String::new() });
            }
        }
        return Some(edits);
    }
    edits.extend(cap_edits);
    let mut written: Vec<String> = cap_params.iter().map(|(n, t)| format!("{n}: {t}")).collect();
    if args_used {
        written.push("args: List[String]".into());
    }
    let open = sig.direct_tokens().into_iter().find(|t| t.text == "(")?;
    let close = sig.direct_tokens().into_iter().find(|t| t.text == ")")?;
    edits.push(Edit { start: tok_end(open), end: (close.line, close.col), text: written.join(", ") });
    // the import names the capability types instead of `Host`, unless `Host` is still used elsewhere
    let imp = host_import?;
    let imp_node = root.kids().into_iter().filter_map(|c| c.node()).find(|n| n.name() == "import" && std::ptr::eq(n.find("dottedPath").map_or(std::ptr::null(), |d| d.tokens()[0] as *const Token), imp.token as *const Token))?;
    let il = imp_node.find("identList")?;
    let removed_host_line = cap_edits_host_lines(body, &body_toks);
    let host_elsewhere = toks.iter().any(|t| t.text == "Host" && t.kind == Kind::Upper && !removed_host_line.contains(&t.line) && !il.tokens().iter().any(|x| std::ptr::eq(*x, t)));
    let mut members: Vec<String> = imp.members.clone().unwrap_or_default();
    if !host_elsewhere {
        members.retain(|m| m != "Host");
    }
    let record_elsewhere = toks.iter().any(|t| t.text == "Capabilities" && t.kind == Kind::Upper && !removed_host_line.contains(&t.line) && !il.tokens().iter().any(|x| std::ptr::eq(*x, t)));
    if !record_elsewhere {
        members.retain(|m| m != "Capabilities");
    }
    for (_, t) in &cap_params {
        if !members.iter().any(|m| m == t) {
            members.push(t.clone());
        }
    }
    let il_toks = il.tokens();
    let (first, last) = (il_toks.first()?, il_toks.last()?);
    if members.is_empty() {
        let its = imp_node.tokens();
        edits.push(line_of_tokens(toks, its.first()?, its.last()?)?);
    } else {
        edits.push(Edit { start: (first.line, first.col), end: tok_end(last), text: members.join(", ") });
    }
    Some(edits)
}

/// Whether the source text `s` (an interpolation hole) names `name` as a word.
fn mentions(s: &str, name: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(j) = s[i..].find(name) {
        let at = i + j;
        let before = at == 0 || !(b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_');
        let end = at + name.len();
        let after = end >= b.len() || !(b[end].is_ascii_alphanumeric() || b[end] == b'_');
        if before && after {
            return true;
        }
        i = at + 1;
    }
    false
}

/// The lines of `main`'s body that a capability rewrite removes (the creation and destructurings), to tell whether
/// `Host` is still named elsewhere.
fn cap_edits_host_lines(body: N, _toks: &[&Token]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for st in body.child_nodes("statement") {
        let ts = st.tokens();
        let creation = ts.iter().any(|t| t.text == "Host") && ts.iter().any(|t| t.text == "capabilities");
        let destructuring = st.child_node("letStmt").and_then(|ls| ls.child_node("pattern")).and_then(|p| p.first_token()).map_or(false, |t| t.text == "Capabilities");
        if creation || destructuring {
            out.extend(ts.iter().map(|t| t.line));
        }
    }
    out
}

/// The capability parameters (name, type) and the body's edits, or None when the body's use of the capabilities
/// record is not one this rewrite keeps the meaning of.
fn caps_rewrite(root: N, body: N, all: &[Token], body_toks: &[&Token], host_imported: bool) -> Option<(Vec<(String, String)>, Vec<Edit>)> {
    if !host_imported {
        return None;
    }
    // `let caps = Host.capabilities()`: a direct statement of the body, immutable, unannotated
    let mut creation: Option<(N, String)> = None;
    for st in body.child_nodes("statement") {
        let Some(ls) = st.child_node("letStmt") else { continue };
        let ts = ls.direct_tokens();
        if ts.len() != 3 || ts[0].text != "let" || ts[1].kind != Kind::Lower || ts[2].text != "=" {
            continue;
        }
        let init = ls.child_node("expr")?;
        if words(init) == "Host.capabilities()" {
            if creation.is_some() {
                return None;
            }
            creation = Some((st, ts[1].text.clone()));
        }
    }
    let (create_st, caps) = creation?;
    let decl_names: BTreeSet<String> = declarations(root).into_iter().filter_map(|d| d.name).collect();
    if CAPABILITY_TYPES.iter().any(|t| decl_names.contains(*t)) {
        return None; // a module-level declaration would take a capability type's name
    }
    let create_toks = create_st.tokens();
    let mut edits = vec![line_of_tokens(all, create_toks.first()?, create_toks.last()?)?];
    // every other use of `caps`: `caps.field` or a `let Capabilities(...) = move caps` destructuring
    let mut params: Vec<(String, String)> = Vec::new();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new(); // a field reached as `caps.field` -> its parameter
    let mut destructured: Vec<(String, String)> = Vec::new(); // (binding, type)
    let mut destructure_lines: BTreeSet<usize> = BTreeSet::new();
    for st in body.child_nodes("statement") {
        let Some(ls) = st.child_node("letStmt") else { continue };
        let Some(pat) = ls.child_node("pattern") else { continue };
        let pts = ls.direct_tokens();
        let moved = pts.iter().any(|t| t.text == "move") && ls.child_node("place").map_or(false, |p| words(p) == caps);
        if !moved {
            continue;
        }
        if pat.first_token().map(|t| t.text.as_str()) != Some("Capabilities") {
            return None;
        }
        // `field: binding` pairs
        let ptoks = pat.tokens();
        let mut i = 0;
        while i + 2 < ptoks.len() {
            if ptoks[i].kind == Kind::Lower && ptoks[i + 1].text == ":" && ptoks[i + 2].kind == Kind::Lower {
                let ty = field_type(&ptoks[i].text)?;
                if ptoks[i + 2].text != "_" {
                    destructured.push((ptoks[i + 2].text.clone(), ty.to_string()));
                }
                i += 3;
            } else {
                i += 1;
            }
        }
        let sts = st.tokens();
        edits.push(line_of_tokens(all, sts.first()?, sts.last()?)?);
        destructure_lines.extend(sts.iter().map(|t| t.line));
    }
    // the uses, in order: each `caps . field` becomes the field's parameter
    let removed: BTreeSet<usize> = create_toks.iter().map(|t| t.line).chain(destructure_lines.iter().copied()).collect();
    let mut order: Vec<String> = Vec::new();
    for (i, t) in body_toks.iter().enumerate() {
        if removed.contains(&t.line) {
            continue;
        }
        if t.kind == Kind::Str && t.interps.iter().any(|(s, _, _)| mentions(s, &caps)) {
            return None;
        }
        if t.kind != Kind::Lower || t.text != caps {
            continue;
        }
        if i > 0 && matches!(body_toks[i - 1].text.as_str(), "." | "?.") {
            continue; // a member named like the record, not the record
        }
        let dot = body_toks.get(i + 1)?;
        let field = body_toks.get(i + 2)?;
        if dot.text != "." || field.kind != Kind::Lower {
            return None;
        }
        let ty = field_type(&field.text)?;
        let pname = field.text.clone();
        if !renamed.contains_key(&pname) {
            renamed.insert(pname.clone(), ty.to_string());
            order.push(pname.clone());
        }
        edits.push(Edit { start: (t.line, t.col), end: tok_end(field), text: pname });
    }
    // the destructured bindings that are used become parameters under their own names
    for (b, ty) in &destructured {
        let used = body_toks.iter().any(|t| t.kind == Kind::Lower && t.text == *b && !removed.contains(&t.line))
            || body_toks.iter().any(|t| t.kind == Kind::Str && t.interps.iter().any(|(s, _, _)| mentions(s, b)));
        if used {
            if renamed.contains_key(b) {
                return None;
            }
            renamed.insert(b.clone(), ty.clone());
            order.push(b.clone());
        }
    }
    // a new parameter's name may not be one the body already binds or reads for something else, nor a module-level name
    for p in &order {
        if p == "args" || decl_names.contains(p) {
            return None;
        }
        let field_use = |i: usize| -> bool { i >= 2 && body_toks[i - 1].text == "." && body_toks[i - 2].text == caps };
        let other = body_toks.iter().enumerate().any(|(i, t)| t.kind == Kind::Lower && t.text == *p && !removed.contains(&t.line) && !field_use(i) && !(i > 0 && matches!(body_toks[i - 1].text.as_str(), "." | "?.")) && !destructured.iter().any(|(b, _)| b == p));
        if other {
            return None;
        }
    }
    for p in order {
        let ty = renamed[&p].clone();
        params.push((p, ty));
    }
    if params.iter().map(|(_, t)| t).collect::<BTreeSet<_>>().len() != params.len() {
        return None; // two parameters of one type: the runtime supplies each type once (4.6)
    }
    Some((params, edits))
}

/// Where the `or fail` a test rewrite removes is implied after it (14): at the call named at this token, or in the
/// checked region with this operator.
#[derive(Clone, Copy)]
enum Implied {
    Call((usize, usize)),
    Region((usize, usize)),
}

/// A handler R7 removes: its outcome expression and left operand, the edit taking out ` or fail`, the edits taking
/// out parentheses `( … )` around the outcome expression alone, and where the implied `or fail` takes over.
struct Removal<'a> {
    outcome: usize,
    left: N<'a>,
    handler: Edit,
    parens: Option<[Edit; 2]>,
    implied: Implied,
}

/// The tests' handlers (R7): `or fail` after a call or integer arithmetic in a test or property body, outside lambdas.
fn tests(root: N) -> Vec<Removal> {
    let mut out = Vec::new();
    for d in declarations(root) {
        if d.kind != "test" && d.kind != "property" {
            continue;
        }
        let Some(blk) = d.node.child_nodes("block").last().copied() else { continue };
        let mut parens: BTreeMap<usize, [Edit; 2]> = BTreeMap::new();
        walk_outside_lambdas(blk, &mut |n| {
            let k = n.kids();
            if n.name() == "primaryExpr" && k.len() == 3 && k[0].text_is("(") && k[2].text_is(")") {
                if let (Some(oe), Some(open), Some(close)) = (k[1].node().and_then(|e| single_to(e, "outcomeExpr")), k[0].tok(), k[2].tok()) {
                    let gone = |t: &Token| Edit { start: (t.line, t.col), end: tok_end(t), text: String::new() };
                    parens.insert(oe.id(), [gone(open), gone(close)]);
                }
            }
        });
        walk_outside_lambdas(blk, &mut |n| {
            if n.name() == "outcomeExpr" {
                if let Some((handler, left, implied)) = or_fail_edit(n) {
                    out.push(Removal { outcome: n.id(), left, handler, parens: parens.get(&n.id()).cloned(), implied });
                }
            }
        });
    }
    out
}

fn single_to<'a>(n: N<'a>, rule: &str) -> Option<N<'a>> {
    let mut x = n;
    while x.name() != rule {
        if x.nkids() != 1 {
            return None;
        }
        x = x.kid(0)?.node()?;
    }
    Some(x)
}

/// The edit that removes ` or fail` from `X or fail` when X is a call, or integer arithmetic (a checked region, 6.4),
/// with X and where its implied `or fail` is to be.
fn or_fail_edit(oe: N) -> Option<(Edit, N, Implied)> {
    let k = oe.kids();
    if k.len() != 3 || !k[1].text_is("or") || !k[2].text_is("fail") {
        return None;
    }
    let left = k[0].node()?;
    let implied = match call_name_token(left) {
        Some(t) => Implied::Call((t.line, t.col)),
        None => Implied::Region(*fortis_types::typer::expr::region_operators(C::N(left)).iter().next()?),
    };
    let last = left.tokens().into_iter().last()?;
    let fail = k[2].tok()?;
    Some((Edit { start: tok_end(last), end: tok_end(fail), text: String::new() }, left, implied))
}

fn walk_outside_lambdas<'a>(n: N<'a>, f: &mut dyn FnMut(N<'a>)) {
    if n.name() == "lambda" {
        return;
    }
    f(n);
    for c in n.kids() {
        if let C::N(m) = c {
            walk_outside_lambdas(m, f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejected_entry_module_keeps_its_main() {
        // the module's own `List` makes `args: List[String]` the wrong form; dropping the unread `args` would accept it
        let src = "export class List[T] {}\n\nexport func main(args: List[String]) Void {}\n";
        assert_eq!(migrate(src, true, &facts_alone("t.ft")), src);
    }

    #[test]
    fn an_accepted_entry_module_loses_export_and_args() {
        let src = "export func main(args: List[String]) Void {\n}\n";
        assert_eq!(migrate(src, true, &facts_alone("t.ft")), "func main() Void {\n}\n");
    }

    #[test]
    fn a_removed_handler_keeps_its_grouping() {
        // D-324: `(a + b or fail) * c` is `(a + b) * c`, never `a + b * c`; parentheses around a call alone go
        let src = "func hide(n: Int) Int => n\n\ntest \"t\" {\n  let a = hide(2)\n  let b = hide(3)\n  let n = (a + b or fail) * 4\n  expect (n + 1 or fail) == 21\n}\n";
        let want = "func hide(n: Int) Int => n\n\ntest \"t\" {\n  let a = hide(2)\n  let b = hide(3)\n  let n = (a + b) * 4\n  expect n + 1 == 21\n}\n";
        assert_eq!(migrate(src, false, &facts_alone("t.ft")), want);
    }

    #[test]
    fn a_lookup_is_rewritten_only_on_the_preludes_string_map() {
        // D-329: a class's own `get`, and a module's own `String`, keep the argument as written
        let own_get = "class Lookup {\n  export func get(self, n: String) Int => n.count()\n}\nfunc f(l: Lookup, key: String) Int {\n  return l.get(copy key)\n}\n";
        assert_eq!(migrate(own_get, false, &facts_alone("t.ft")), own_get);
        let map = "func f(m: Map[String, Int], key: String) Int {\n  return m.get(copy key) ?? 0\n}\n";
        assert_eq!(migrate(map, false, &facts_alone("t.ft")), "func f(m: Map[String, Int], key: String) Int {\n  return m.get(key) ?? 0\n}\n");
    }

    #[test]
    fn a_position_moves_with_the_edits_before_it() {
        let e = |s: (usize, usize), en: (usize, usize), t: &str| Edit { start: s, end: en, text: t.into() };
        // `(f() or fail) == 2` loses `(`, ` or fail` and `)`: `f` moves one column left; a later line keeps its column
        let edits = vec![e((2, 3), (2, 4), ""), e((2, 7), (2, 15), ""), e((2, 15), (2, 16), "")];
        assert_eq!(moved((2, 4), &edits), (2, 3));
        assert_eq!(moved((2, 17), &edits), (2, 7));
        assert_eq!(moved((3, 5), &edits), (3, 5));
        // a deleted line moves the lines after it up
        assert_eq!(moved((5, 2), &[e((1, 1), (2, 1), "")]), (4, 2));
    }
}
