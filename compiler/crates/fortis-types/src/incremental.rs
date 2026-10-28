//! A module check run one unit at a time (DEL-46), for the query database.
//!
//! The batch check (`check::check_parsed`) passes over a module's units twice. A unit is a declaration, an
//! `implement` block, or a test (`Typer::units`). The first pass computes effect summaries in unit order, and its
//! records are discarded. The final pass checks every unit against the summaries it finds: final-pass summaries of
//! earlier units, first-pass summaries of later ones. Here, one unit runs at a time against summaries the caller
//! gives. The run reports the unit's records, the summaries it produced, and the summary labels it looked up, so a
//! caller can track dependencies exactly and reproduce the batch result (`from_scratch`).

use std::collections::{BTreeMap, BTreeSet};

use fortis_diag::{Json, Record};
use fortis_names::{Libraries, Library, Resolver};
use fortis_syntax::view::N;
use fortis_syntax::Parsed;

use crate::decls::{DeclChecker, Session};
use crate::effects::Summary;
use crate::typer::{Typer, Unit};

/// The records of a module that come before its units: the resolver's, the declaration checker's and the
/// final pass's import checks; and the units in order.
pub struct Interface {
    pub resolver: Vec<Record>,
    pub declarations: Vec<Record>,
    pub imports: Vec<Record>,
    pub units: Vec<Unit>,
}

/// What a unit's result depends on besides summaries: the module's skeleton (every token outside a function,
/// method, cleanup or test body, with its position: the declaration table and the imports) and each unit's own
/// tokens with their positions.
#[derive(Clone, Debug)]
pub struct Texts {
    pub skeleton: String,
    pub units: BTreeMap<String, String>,
}

fn toks_text(n: N, out: &mut String, skip_bodies: bool) {
    for c in n.kids() {
        match c {
            fortis_syntax::view::C::T(t) => {
                if !matches!(t.kind, fortis_syntax::lexer::Kind::Nl | fortis_syntax::lexer::Kind::Newline) {
                    out.push_str(&format!("{}:{}:{} ", t.line, t.col, t.text));
                }
            }
            fortis_syntax::view::C::N(m) => {
                let body = matches!(m.name(), "funcBody" | "cleanupDecl") || (m.name() == "block" && matches!(n.name(), "testDecl" | "propertyDecl"));
                if skip_bodies && body {
                    out.push_str("{…} ");
                } else {
                    toks_text(m, out, skip_bodies);
                }
            }
        }
    }
}

pub fn texts(parsed: &Parsed, file: &str, module: &str) -> Texts {
    with_typer(parsed, file, module, |ty, mtree| texts_of(ty, mtree)).1
}

/// Each unit of the module with its node (none when the table lost it).
pub fn unit_nodes<'b>(ty: &Typer<'b>, mtree: N<'b>) -> Vec<(Unit, Option<N<'b>>)> {
    let decls = fortis_syntax::view::declarations(mtree);
    ty.units(mtree)
        .into_iter()
        .map(|u| {
            let node = match &u {
                Unit::Decl(name) => ty.dc.table.decls.values().find(|d| d.module == ty.module && d.name == *name).and_then(|d| d.node),
                Unit::Implement(i, _) => ty.dc.table.implement_blocks.get(*i).map(|b| b.3),
                Unit::Test(i, _) => decls.get(*i).map(|d| d.node),
            };
            (u, node)
        })
        .collect()
}

/// `texts` with a checker already loaded for the module.
pub fn texts_of<'b>(ty: &Typer<'b>, mtree: N<'b>) -> Texts {
    let mut skeleton = String::new();
    toks_text(mtree, &mut skeleton, true);
    let mut units = BTreeMap::new();
    for (u, node) in unit_nodes(ty, mtree) {
        let mut t = String::new();
        if let Some(n) = node {
            toks_text(n, &mut t, false);
        }
        units.insert(u.id(), t);
    }
    Texts { skeleton, units }
}

/// The summary labels a unit can produce (besides `closure`, which any unit with a lambda sets).
pub fn labels(parsed: &Parsed, file: &str, module: &str) -> BTreeMap<String, String> {
    with_typer(parsed, file, module, |ty, mtree| labels_of(ty, mtree)).1
}

/// `labels` with a checker already loaded for the module.
pub fn labels_of<'b>(ty: &mut Typer<'b>, mtree: N<'b>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for u in ty.units(mtree) {
        let id = u.id();
        match &u {
            Unit::Decl(name) => {
                if let Some(d) = ty.dc.table.decls.values().find(|d| d.module == ty.module && d.name == *name) {
                    match d.kind.as_str() {
                        "func" => {
                            out.insert(format!("function {name}"), id.clone());
                        }
                        "class" | "enum" => {
                            for m in d.methods.keys() {
                                out.insert(format!("method {name}.{m}"), id.clone());
                            }
                            out.insert(format!("cleanup {name}"), id.clone());
                        }
                        _ => {}
                    }
                }
            }
            Unit::Implement(i, _) => {
                if let Some((_, target, _, node)) = ty.dc.table.implement_blocks.get(*i).cloned() {
                    for m in node.child_nodes("implementMember") {
                        if let Some(head) = m.find("methodHead") {
                            let sig = ty.dc.signature(head, true, m.find("funcBody"));
                            out.insert(format!("method {}.{}", target.name, sig.name), id.clone());
                        }
                    }
                }
            }
            Unit::Test(_, label) => {
                let name = label.split_once(' ').map_or("", |x| x.1);
                out.insert(format!("test {name}"), id.clone());
            }
        }
    }
    out
}

/// One unit's run.
pub struct UnitOut {
    pub records: Vec<Record>,
    pub produced: BTreeMap<String, Summary>,
    pub requested: BTreeSet<String>,
}

fn with_typer<R>(parsed: &Parsed, file: &str, module: &str, f: impl for<'b> FnOnce(&mut Typer<'b>, N<'b>) -> R) -> (Vec<Record>, R) {
    let libs = Libraries { libs: vec![Library::new()] };
    let sess = Session::new(&libs);
    let mtree = N::new(&parsed.root, &parsed.toks);
    // the module key "main" is a module checked on its own: its entry `main` is judged as `check` judges it (D-51.2)
    let resolver = Resolver::new(&sess.libs.libs[0], file, module == "main").resolve_module(mtree);
    let mut dc = DeclChecker::new(&sess, 0, file, module, "");
    dc.load(mtree, false);
    dc.check();
    let mut ty = Typer::new(dc);
    let r = f(&mut ty, mtree);
    (resolver, r)
}

pub fn interface(parsed: &Parsed, file: &str, module: &str) -> Interface {
    let (resolver, (declarations, imports, units)) = with_typer(parsed, file, module, |ty, mtree| interface_of(ty, mtree));
    Interface { resolver, declarations, imports, units }
}

/// The declaration checker's records, the final pass's import checks and the units, with a checker already loaded
/// for the module.
pub fn interface_of<'b>(ty: &mut Typer<'b>, mtree: N<'b>) -> (Vec<Record>, Vec<Record>, Vec<Unit>) {
    let declarations = ty.dc.records.clone();
    let n = ty.nrecords();
    ty.final_pass = true;
    ty.check_imports(mtree);
    let imports = ty.dc.records[n..].to_vec();
    (declarations, imports, ty.units(mtree))
}

/// Run one unit against the given summaries (first pass, or final pass when `final_pass`).
pub fn run_unit(parsed: &Parsed, file: &str, module: &str, unit: &Unit, given: &BTreeMap<String, Summary>, final_pass: bool) -> UnitOut {
    with_typer(parsed, file, module, |ty, mtree| run_unit_of(ty, mtree, unit, given, final_pass)).1
}

/// `run_unit` on a fresh typer over a checker already loaded for the module.
pub fn run_unit_of<'b>(ty: &mut Typer<'b>, mtree: N<'b>, unit: &Unit, given: &BTreeMap<String, Summary>, final_pass: bool) -> UnitOut {
    ty.final_pass = false;
    ty.check_imports(mtree);
    ty.final_pass = final_pass;
    let n = ty.nrecords();
    ty.summaries = given.clone();
    ty.requested.borrow_mut().clear();
    ty.run_unit(mtree, unit);
    let records = ty.dc.records[n..].to_vec();
    let produced: BTreeMap<String, Summary> = ty.summaries.iter().filter(|(k, v)| given.get(*k).map_or(true, |g| summary_json(g) != summary_json(v))).map(|(k, v)| (k.clone(), v.clone())).collect();
    let requested = ty.requested.borrow().clone();
    UnitOut { records, produced, requested }
}

/// Records as the batch check keeps them: a record equal in code, line and message to an earlier one is dropped.
pub fn dedupe(records: Vec<Record>) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::new();
    for r in records {
        if !out.iter().any(|x| x.code == r.code && x.line == r.line && x.message == r.message) {
            out.push(r);
        }
    }
    out
}

/// The batch result reproduced unit by unit, without a database: the reference for the query database.
pub fn from_scratch(parsed: &Parsed, file: &str, module: &str) -> Vec<Record> {
    let i = interface(parsed, file, module);
    let mut sums: BTreeMap<String, Summary> = BTreeMap::new();
    for u in &i.units {
        let out = run_unit(parsed, file, module, u, &sums, false);
        sums.extend(out.produced);
    }
    let mut body = i.declarations.clone();
    body.extend(i.imports.clone());
    for u in &i.units {
        let out = run_unit(parsed, file, module, u, &sums, true);
        body.extend(out.records);
        sums.extend(out.produced);
    }
    let mut all = i.resolver.clone();
    all.extend(dedupe(body));
    all
}

/// A summary as JSON (the query database persists summaries).
pub fn summary_json(s: &Summary) -> Json {
    let sites: Vec<Json> = s
        .sites
        .iter()
        .map(|(k, t)| Json::obj([("effect", Json::str(k)), ("kind", Json::str(t.kind.as_str())), ("text", Json::str(&t.text)), ("line", Json::Int(t.line as i64)), ("col", Json::Int(t.col as i64))]))
        .collect();
    Json::obj([
        ("effects", Json::Arr(s.effects.iter().map(|(e, l)| Json::Arr(vec![Json::str(e), Json::str(l)])).collect())),
        ("footprint", Json::strs(s.footprint.iter().cloned().collect::<Vec<String>>())),
        ("authorities", Json::strs(s.authorities.iter().cloned().collect::<Vec<String>>())),
        ("needs_auth", Json::Bool(s.needs_auth)),
        ("sites", Json::Arr(sites)),
        ("uncertain", Json::Obj(s.uncertain.iter().map(|(k, v)| (k.clone(), Json::str(v))).collect())),
    ])
}

pub fn summary_from_json(j: &Json) -> Option<Summary> {
    use fortis_syntax::lexer::{Kind, Token};
    let strs = |k: &str| -> Option<Vec<String>> {
        match j.get(k)? {
            Json::Arr(v) => v.iter().map(|x| if let Json::Str(s) = x { Some(s.clone()) } else { None }).collect(),
            _ => None,
        }
    };
    let mut s = Summary::default();
    if let Json::Arr(es) = j.get("effects")? {
        for e in es {
            if let Json::Arr(p) = e {
                if let (Some(Json::Str(a)), Some(Json::Str(b))) = (p.first(), p.get(1)) {
                    s.effects.push((a.clone(), b.clone()));
                }
            }
        }
    }
    s.footprint = strs("footprint")?.into_iter().collect();
    if let Some(Json::Obj(u)) = j.get("uncertain") {
        s.uncertain = u.iter().filter_map(|(k, v)| if let Json::Str(x) = v { Some((k.clone(), x.clone())) } else { None }).collect();
    }
    s.authorities = strs("authorities")?.into_iter().collect();
    s.needs_auth = matches!(j.get("needs_auth")?, Json::Bool(true));
    if let Json::Arr(sites) = j.get("sites")? {
        for x in sites {
            let kind = match x.get("kind")? {
                Json::Str(k) => k.clone(),
                _ => return None,
            };
            let kind = [Kind::Newline, Kind::Nl, Kind::Lower, Kind::Upper, Kind::Kw, Kind::Int, Kind::Float, Kind::Dec, Kind::Dur, Kind::Str, Kind::Raw, Kind::Bytes, Kind::Template, Kind::Punct, Kind::RefOpen, Kind::RefClose, Kind::Eof]
                .into_iter()
                .find(|k| k.as_str() == kind)?;
            let (Json::Str(e), Json::Str(t), Json::Int(l), Json::Int(c)) = (x.get("effect")?, x.get("text")?, x.get("line")?, x.get("col")?) else { return None };
            s.sites.insert(e.clone(), Token::new(kind, t, *l as usize, *c as usize));
        }
    }
    Some(s)
}
