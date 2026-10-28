//! Name resolution over parse trees (Language Reference 4.3-4.5; DEL-06/DEL-07): a port of the `Resolver` of
//! conformance/tools/names.py. Scopes live in an arena; a lambda's scope records the scope enclosing it, so an
//! uncaptured outer name is E-FUN-011 rather than E-NAM-001.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use fortis_diag::{Json, Record};
use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{declarations, imports, DeclInfo, C, N};
use fortis_syntax::{parse_from, Parsed};

use crate::library::{members_of, Library, LibraryModule};

/// The entry point's modifiers (LR 4.6): the four forms, each of which may also carry `recovers` (X-155); the
/// grammar's modifier order is async, try, recovers.
pub const ENTRY_FORMS: &[&str] = &["", "async", "try", "async try", "recovers", "async recovers", "try recovers", "async try recovers"];
const ENTRY_SIGNATURE: &str = "( args : List [ String ] ) Void";

/// The declaration heads whose kit this toolchain does not install (LR 16, Platform 1.2; X-231): (head word,
/// kit, the package that defines it). The Dependency injection kit's `context` is installed (fortis-di).
const UNINSTALLED_HEADS: &[(&str, &str, &str)] = &[
    ("application", "Runtime host", "fortis.runtime"),
    ("model", "Data", "fortis.data"),
    ("explore", "Developer tooling", ""),
    ("view", "Web UI", "fortis.web.ssr"),
    ("component", "Web UI", "fortis.web.components"),
];

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub kind: String,
    pub members: BTreeSet<String>,
    pub origin: String,
    pub module: Option<String>,
    pub has_module: bool,
    pub path: String,
}

fn info(kind: &str) -> Info {
    Info { kind: kind.to_string(), ..Default::default() }
}

struct Scope {
    parent: Option<usize>,
    names: BTreeMap<String, Info>,
    lambda_outer: Option<usize>,
}

pub struct Resolver<'l> {
    lib: &'l Library,
    file: String,
    /// The module is checked on its own, as the entry module of an implicit one-module application (D-51.2);
    /// a module of a package is judged by the manifest's `entry` instead (resolve.rs `check_entry`).
    entry: bool,
    pub records: Vec<Record>,
    scopes: Vec<Scope>,
    module_scope: usize,
    enum_variants: BTreeSet<String>,
}

impl<'l> Resolver<'l> {
    pub fn new(lib: &'l Library, file: &str, entry: bool) -> Resolver<'l> {
        Resolver { lib, file: file.to_string(), entry, records: vec![], scopes: vec![], module_scope: 0, enum_variants: BTreeSet::new() }
    }

    fn rec(&mut self, code: &str, msg: String, rule: &str, tok: &Token, fields: Json) {
        self.records.push(Record::new(code, msg, rule, &self.file, tok.line, tok.col, fields));
    }

    fn nf(name: &str, candidates: Vec<String>) -> Json {
        Json::obj([("name", Json::str(name)), ("candidates", Json::strs(candidates))])
    }

    /// The candidates of a member a module does not declare: the members another module can name, its exported ones.
    fn exported(lm: &LibraryModule) -> Vec<String> {
        lm.decls.iter().filter(|(_, d)| d.exported).map(|(k, _)| k.clone()).collect()
    }

    fn scope(&mut self, parent: Option<usize>) -> usize {
        self.scopes.push(Scope { parent, names: BTreeMap::new(), lambda_outer: None });
        self.scopes.len() - 1
    }

    fn lookup(&self, mut s: usize, n: &str) -> Option<&Info> {
        loop {
            if let Some(i) = self.scopes[s].names.get(n) {
                return Some(i);
            }
            match self.scopes[s].parent {
                Some(p) => s = p,
                None => return None,
            }
        }
    }

    /// `n`, looked up from scope `s`, is a module-level name: a declaration, an import, a module alias or the prelude.
    fn module_level(&self, mut s: usize, n: &str) -> bool {
        loop {
            if self.scopes[s].names.contains_key(n) {
                return s == self.module_scope;
            }
            match self.scopes[s].parent {
                Some(p) => s = p,
                None => return false,
            }
        }
    }

    fn declare(&mut self, s: usize, n: &str, i: Info) {
        self.scopes[s].names.insert(n.to_string(), i);
    }

    /// An import binds `n` (LR 4.3): a name an earlier import has bound is rejected, as the package resolver
    /// rejects it (resolve.rs `check_module`); a later local declaration that collides is the declarations' check.
    fn bind_import(&mut self, bound: &mut BTreeMap<String, String>, n: &str, path: &str, tok: &Token) {
        if let Some(prev) = bound.get(n).cloned() {
            // the candidate is the first binding: a later import of the same name does not replace it (LR 4.3)
            self.rec("E-NAM-001", format!("import binds {n} twice or collides with a declaration"), "LANG-04-03", tok, Self::nf(n, vec![prev]));
            return;
        }
        bound.insert(n.to_string(), path.to_string());
    }

    /// A module checked on its own is the entry module of an implicit one-module application (`fortis run
    /// FILE.ft` runs its `main`): an exported `main` it declares must have one of the four forms of LR 4.6, each
    /// of which may also carry `recovers` (X-155). A module of a package is judged by the manifest's `entry`
    /// instead (resolve.rs `check_entry`); a module with no exported `main` is a library module (D-51.2).
    fn check_entry(&mut self, decls: &[DeclInfo]) {
        if !self.entry {
            return;
        }
        // any exported declaration named main is the entry point (LR 4.6). A module that exports none is a
        // library module. One that exports a non-function is not: the entry must be a function of 4.6.
        let Some(main) = decls.iter().find(|d| d.name.as_deref() == Some("main") && d.exported) else { return };
        if main.kind != "func" {
            let Some(tok) = &main.token else { return };
            self.rec("E-NAM-001", format!("exported main is a {}, not one of the forms of 4.6", main.kind), "LANG-04-06", tok, Self::nf("main", vec!["export [async] [try] [recovers] func main(args: List[String]) Void".into()]));
            return;
        }
        let Some(head) = main.node.find("funcHead") else { return };
        let words = head.find("modifiers").map(|mm| mm.tokens().iter().map(|t| t.text.clone()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let Some(sig) = head.find("funcSignature") else { return };
        let text = sig.tokens().iter().map(|t| t.text.clone()).collect::<Vec<_>>().join(" ");
        let form = "export [async] [try] [recovers] func main(args: List[String]) Void";
        if ENTRY_FORMS.contains(&words.as_str()) && text == ENTRY_SIGNATURE {
            // the token text can match while `List`, `String` or `Void` names a module-level declaration or
            // import. 4.6's forms use the prelude types (D-51.2)
            if let Some(n) = self.entry_type_shadowed() {
                let Some(tok) = &main.token else { return };
                self.rec("E-NAM-001", format!("main's `{n}` is not the prelude {n}, so the signature is not one of the forms of 4.6"), "LANG-04-06", tok, Self::nf("main", vec![form.into()]));
            }
            return;
        }
        let Some(tok) = &main.token else { return };
        self.rec("E-NAM-001", format!("main has signature `{words} func main{text}`, not one of the forms of 4.6"), "LANG-04-06", tok, Self::nf("main", vec![form.into()]));
    }

    /// The first of 4.6's `List`, `String` and `Void` that a module-level binding has replaced. An unbound name is
    /// the prelude's when the prelude is loaded, and a missing prelude is not reported here.
    fn entry_type_shadowed(&self) -> Option<String> {
        let names = &self.scopes[self.module_scope].names;
        for n in ["List", "String", "Void"] {
            if let Some(info) = names.get(n) {
                if info.origin != "prelude" {
                    return Some(n.to_string());
                }
            }
        }
        None
    }

    // ---- module level -----------------------------------------------------
    pub fn resolve_module(&mut self, mtree: N) -> Vec<Record> {
        let m = self.scope(None);
        self.module_scope = m;
        if let Some(p) = self.lib.prelude() {
            for (n, d) in &p.decls {
                self.declare(m, n, Info { kind: d.kind.to_string(), members: d.members.clone(), origin: "prelude".into(), ..Default::default() });
                if d.kind == "enum" {
                    self.enum_variants.extend(d.members.iter().cloned());
                }
            }
        }
        let mut bound: BTreeMap<String, String> = BTreeMap::new();   // the names the imports bind: importing one name twice is an error (LR 4.3, D-51.4)
        for imp in imports(mtree) {
            let path = imp.path.join(".");
            let libmod: Option<Arc<LibraryModule>> = self.lib.modules.get(&path).cloned();
            match &imp.members {
                None => {
                    let alias = imp.alias.clone().unwrap_or_else(|| imp.path.last().unwrap().clone());
                    self.bind_import(&mut bound, &alias, &path, imp.token);
                    self.declare(m, &alias, Info { kind: "module".into(), module: libmod.as_ref().map(|_| path.clone()), has_module: libmod.is_some(), path: path.clone(), ..Default::default() });
                }
                Some(members) => {
                    for mm in members {
                        if let Some(lm) = &libmod {
                            if !lm.decls.contains_key(mm) {
                                let cands = lm.decls.keys().cloned().collect();
                                self.rec("E-NAM-001", format!("{path} declares no {mm}"), "LANG-04-03", imp.token, Self::nf(mm, cands));
                            }
                        }
                        self.bind_import(&mut bound, mm, &path, imp.token);
                        let d = libmod.as_ref().and_then(|lm| lm.decls.get(mm)).cloned();
                        self.declare(
                            m,
                            mm,
                            Info { kind: d.as_ref().map_or("imported".into(), |d| d.kind.to_string()), members: d.as_ref().map_or_else(BTreeSet::new, |d| d.members.clone()), origin: path.clone(), ..Default::default() },
                        );
                        if let Some(d) = d {
                            if d.kind == "enum" {
                                self.enum_variants.extend(d.members.iter().cloned());
                            }
                        }
                    }
                }
            }
        }
        let decls = declarations(mtree);
        for d in &decls {
            if let Some(n) = &d.name {
                if let Some(existing) = self.scopes[m].names.get(n) {
                    if existing.origin != "prelude" {
                        self.rec("E-NAM-001", format!("{n} is declared twice or collides with an import"), "LANG-04-03", d.token.unwrap(), Self::nf(n, vec![]));
                    }
                }
                let members = members_of(d.node);
                if d.kind == "enum" {
                    self.enum_variants.extend(members.iter().cloned());
                }
                self.declare(m, n, Info { kind: d.kind.to_string(), members, origin: "module".into(), ..Default::default() });
            }
        }
        self.check_entry(&decls);
        for d in &decls {
            self.resolve_decl(d);
        }
        std::mem::take(&mut self.records)
    }

    /// A head whose kit this toolchain does not install cannot be validated or elaborated, so it is rejected,
    /// naming the kit (LR 16, E-NAM-034; D-90, X-231). `context` is the Dependency injection kit's, which the
    /// toolchain elaborates before the check (fortis-di).
    fn check_head(&mut self, node: N) {
        let Some(word) = node.child_node("headWord").and_then(|h| h.tokens().first().copied()) else { return };
        let prefix = node.direct_tokens().into_iter().find(|t| ["server", "client", "hybrid"].contains(&t.text.as_str()));
        if let (true, Some(prefix)) = (word.text == "context", prefix) {
            // the installed kit defines `context` without a prefix, and elaborates only that form
            self.rec(
                "E-NAM-034",
                format!(
                    "`{} context` is a head form no installed kit defines: the Dependency injection kit's `context` takes no `server`, `client` or `hybrid` prefix, so the declaration can be neither validated nor elaborated (LR 16)",
                    prefix.text
                ),
                "PROG-DEL-04",
                word,
                Self::nf(&format!("{} context", prefix.text), vec!["context".to_string()]),
            );
            return;
        }
        let Some((_, kit, package)) = UNINSTALLED_HEADS.iter().find(|(w, _, _)| *w == word.text) else { return };
        let place = if package.is_empty() { String::new() } else { format!(" (`{package}`)") };
        self.rec(
            "E-NAM-034",
            format!(
                "`{}` is a head of the {kit} kit{place}, which this toolchain does not install: the declaration can be neither validated nor elaborated, so it is not accepted (LR 16)",
                word.text
            ),
            "PROG-DEL-04",
            word,
            Self::nf(&word.text, if package.is_empty() { vec![] } else { vec![package.to_string()] }),
        );
    }

    fn resolve_decl(&mut self, d: &DeclInfo) {
        let node = d.node;
        let kind = d.kind;
        let ms = self.module_scope;
        let scope = self.scope(Some(ms));
        for tp in node.find_all("typeParams") {
            for t in tp.tokens() {
                if t.kind == Kind::Upper {
                    self.declare(scope, &t.text, info("typeparam"));
                }
            }
        }
        if let Some(lp) = node.find("lifetimeParam") {
            if let Some(t) = lp.tokens().get(2) {
                self.declare(scope, &t.text, info("lifetime"));
            }
        }
        if kind == "head" {
            self.check_head(node);
            return;
        }
        let member_rules: BTreeSet<&str> = ["classMember", "enumMember", "interfaceMember", "mixinMember", "implementMember"].into_iter().collect();
        if ["class", "enum", "interface", "mixin", "implement"].contains(&kind) {
            self.check_types_in(node, scope, &member_rules);
            for rule in ["classMember", "enumMember", "interfaceMember", "mixinMember", "implementMember"] {
                for m in node.child_nodes(rule) {
                    self.resolve_member(m, scope);
                }
            }
            return;
        }
        if kind == "func" {
            self.resolve_callable(node.find("funcHead").unwrap(), node.find("funcBody"), scope, false);
            return;
        }
        if kind == "aspect" {
            self.resolve_aspect(node, scope);
            return;
        }
        if kind == "test" || kind == "property" {
            let s2 = self.scope(Some(scope));
            let ts = node.direct_tokens();
            if let Some(i) = ts.iter().position(|t| t.text == "as") {
                self.declare(s2, &ts[i + 1].text, info("row"));
            }
            if let Some(pl) = node.find("paramList") {
                self.declare_params(pl, s2);
            }
            for b in node.child_nodes("block") {
                self.resolve_block(b, s2);
            }
            for cr in node.find_all("caseRow") {
                if let Some(r) = cr.find("recordLit") {
                    self.resolve_expr(C::N(r), s2);
                }
            }
            return;
        }
        let empty = BTreeSet::new();
        self.check_types_in(node, scope, &empty);
        for c in node.kids() {
            if let Some(n) = c.node() {
                if n.name() == "constExpr" || n.name() == "expr" {
                    self.resolve_expr(c, scope);
                }
            }
        }
    }

    fn resolve_member(&mut self, m: N, scope: usize) {
        let s = self.scope(Some(scope));
        if let Some(head) = m.find("methodHead") {
            let body = m.find("funcBody");
            self.resolve_callable(head, body, s, true);
            return;
        }
        let empty = BTreeSet::new();
        if let Some(v) = m.find("variant") {
            self.check_types_in(v, s, &empty);
            return;
        }
        for fd in m.child_nodes("fieldDecl") {
            self.check_types_in(fd, s, &empty);
            if let Some(ce) = fd.find("constExpr") {
                self.resolve_expr(C::N(ce), s);
            }
        }
        if let Some(cd) = m.find("constDecl") {
            self.check_types_in(cd, s, &empty);
            if let Some(ce) = cd.find("constExpr") {
                self.resolve_expr(C::N(ce), s);
            }
        }
        if let Some(cl) = m.find("cleanupDecl") {
            self.declare(s, "self", info("self"));
            if let Some(b) = cl.find("block") {
                self.resolve_block(b, s);
            }
        }
    }

    fn resolve_aspect(&mut self, node: N, scope: usize) {
        let s = self.scope(Some(scope));
        if let Some(ap) = node.find("aspectParams") {
            self.declare_params(ap, s);
        }
        let empty = BTreeSet::new();
        for part in node.find_all("aspectPart") {
            let ts = part.direct_tokens();
            let words: Vec<&str> = ts.iter().map(|t| t.text.as_str()).collect();
            let s2 = self.scope(Some(s));
            if words.first() == Some(&"needs") && ts.len() > 1 && ts[1].kind == Kind::Lower {
                self.declare(s, &ts[1].text, info("needs"));
                self.check_types_in(part, s, &empty);
            } else if words.first() == Some(&"after") {
                self.declare(s2, &ts[2].text, info("binding"));
            } else if words.first() == Some(&"on") {
                self.declare(s2, &ts[3].text, info("binding"));
            }
            for b in part.child_nodes("block") {
                self.resolve_block(b, s2);
            }
        }
        for al in node.child_nodes("argList") {
            self.resolve_expr(C::N(al), s);
        }
    }

    fn declare_params(&mut self, pl: N, scope: usize) {
        let mut ps = pl.find_all("param");
        ps.extend(pl.find_all("lambdaParam"));
        let empty = BTreeSet::new();
        for p in ps {
            if let Some(rp) = p.find("restParam") {
                self.declare(scope, &rp.tokens()[1].text, info("param"));
                continue;
            }
            let ts: Vec<&Token> = p.direct_tokens().into_iter().filter(|t| t.text != "...").collect();
            if let Some(t0) = ts.first() {
                if t0.kind == Kind::Lower {
                    if p.name() == "lambdaParam" {
                        self.declare_binding(t0, scope, "param");
                    } else {
                        self.declare(scope, &t0.text, info("param"));
                    }
                }
            }
            if p.find("selfParam").is_some() {
                self.declare(scope, "self", info("self"));
            }
            self.check_types_in(p, scope, &empty);
            if let Some(ce) = p.find("constExpr") {
                self.resolve_expr(C::N(ce), scope);
            }
        }
    }

    fn resolve_callable(&mut self, head: N, body: Option<N>, scope: usize, is_method: bool) {
        let s = self.scope(Some(scope));
        for tp in head.find_all("typeParams") {
            for t in tp.tokens() {
                if t.kind == Kind::Upper {
                    self.declare(s, &t.text, info("typeparam"));
                }
            }
        }
        let sig = head.find("funcSignature");
        let pl = sig.and_then(|s| s.find("paramList"));
        if is_method {
            self.declare(s, "self", info("self"));
        }
        if let Some(pl) = pl {
            self.declare_params(pl, s);
        }
        if let Some(mods) = head.find("modifiers") {
            for t in mods.tokens() {
                if t.kind == Kind::Upper && self.lookup(scope, &t.text).is_none() {
                    self.rec("E-NAM-001", format!("contract `{}` is not declared", t.text), "LANG-04-05", t, Self::nf(&t.text, vec![]));
                }
            }
        }
        let empty = BTreeSet::new();
        if let Some(sig) = sig {
            if let Some(rt) = sig.find("returnType") {
                self.check_types_in(rt, s, &empty);
            }
            if let Some(wc) = sig.find("whereClause") {
                self.check_types_in(wc, s, &empty);
            }
        }
        if let Some(body) = body {
            let b = body.kid(0).and_then(|c| c.node()).filter(|n| n.name() == "block");
            if let Some(b) = b {
                self.resolve_block(b, s);
            } else if let Some(e) = body.find("expr") {
                self.resolve_expr(C::N(e), s);
            }
        }
    }

    // ---- statements -------------------------------------------------------
    fn resolve_block(&mut self, block: N, scope: usize) {
        let s = self.scope(Some(scope));
        for st in block.child_nodes("statement") {
            self.resolve_statement(st, s);
        }
    }

    fn declare_binding(&mut self, tok: &Token, scope: usize, kind: &str) {
        if let Some(existing) = self.lookup(scope, &tok.text) {
            if existing.kind != "prelude" && existing.origin != "prelude" {
                let k = if existing.kind.is_empty() { "?".to_string() } else { existing.kind.clone() };
                self.rec("E-NAM-003", format!("`{}` shadows a visible binding", tok.text), "LANG-04-05", tok, Self::nf(&tok.text, vec![k]));
            }
        }
        self.declare(scope, &tok.text, info(kind));
    }

    fn resolve_statement(&mut self, st: N, scope: usize) {
        let inner = st.kid(0).unwrap();
        let empty = BTreeSet::new();
        let inner = match inner {
            C::T(_) => {
                for c in st.kids() {
                    if c.is("expr") {
                        self.resolve_expr(c, scope);
                    }
                }
                let ts = st.direct_tokens();
                let words: Vec<&str> = ts.iter().map(|t| t.text.as_str()).collect();
                if words.first() == Some(&"report") {
                    if let Some(i) = words.iter().position(|w| *w == "as") {
                        self.declare_binding(ts[i + 1], scope, "receipt");
                    }
                }
                return;
            }
            C::N(n) => n,
        };
        match inner.name() {
            "letStmt" => {
                let ts = inner.direct_tokens();
                let pat = inner.child_node("pattern");
                for c in inner.kids() {
                    if c.is("expr") {
                        self.resolve_expr(c, scope);
                    } else if c.is("type") {
                        self.check_types_in(c.node().unwrap(), scope, &empty);
                    }
                }
                if let Some(pl) = inner.child_node("place") {
                    self.resolve_place(pl, scope);
                }
                if let Some(pat) = pat {
                    for b in pattern_bindings(pat) {
                        self.declare_binding(b, scope, "let");
                    }
                } else if let Some(nm) = ts.iter().find(|t| t.kind == Kind::Lower) {
                    self.declare_binding(nm, scope, "let");
                }
            }
            "assignStmt" => {
                if let Some(pl) = inner.find("place") {
                    self.resolve_place(pl, scope);
                }
                if let Some(e) = inner.find("expr") {
                    self.resolve_expr(C::N(e), scope);
                }
            }
            "ifStmt" => {
                for c in inner.kids() {
                    if c.is("expr") {
                        self.resolve_expr(c, scope);
                    } else if c.is("block") {
                        self.resolve_block(c.node().unwrap(), scope);
                    }
                }
            }
            "whileStmt" | "loopStmt" | "deferStmt" => {
                if inner.name() == "whileStmt" {
                    if let Some(e) = inner.find("expr") {
                        self.resolve_expr(C::N(e), scope);
                    }
                }
                if let Some(b) = inner.find("block") {
                    self.resolve_block(b, scope);
                }
            }
            "forStmt" => {
                let ts = inner.direct_tokens();
                let var = ts[ts.iter().position(|t| t.text == "for").unwrap() + 1];
                for c in inner.kids() {
                    if c.is("expr") || c.is("rangeExpr") {
                        self.resolve_expr(c, scope);
                    } else if c.is("place") {
                        self.resolve_place(c.node().unwrap(), scope);
                    }
                }
                let s2 = self.scope(Some(scope));
                if var.text != "_" {
                    self.declare_binding(var, s2, "for");
                }
                if let Some(b) = inner.find("block") {
                    self.resolve_block(b, s2);
                }
            }
            "scopeStmt" => {
                let s2 = self.scope(Some(scope));
                let name = inner.direct_tokens()[1].text.clone();
                self.declare(s2, &name, info("scope"));
                if let Some(b) = inner.find("block") {
                    self.resolve_block(b, s2);
                }
            }
            "captureStmt" => {
                if let Some(e) = inner.find("expr") {
                    self.resolve_expr(C::N(e), scope);
                }
            }
            "recordStmt" => {
                if let Some(al) = inner.find("argList") {
                    self.resolve_expr(C::N(al), scope);
                }
            }
            "block" => self.resolve_block(inner, scope),
            "exprStmt" => self.resolve_expr(C::N(inner), scope),
            _ => {
                for c in inner.kids() {
                    if c.is("expr") {
                        self.resolve_expr(c, scope);
                    }
                }
            }
        }
    }

    // ---- expressions ---------------------------------------------------------
    fn resolve_place(&mut self, place: N, scope: usize) {
        let ts = place.direct_tokens();
        let root = ts[0];
        if root.text == "self" {
            if self.lookup(scope, "self").is_none() {
                self.rec("E-NAM-001", "`self` outside a method".into(), "LANG-04-05", root, Self::nf("self", vec![]));
            }
            return;
        }
        let info = self.check_root(root, scope);
        let kids = place.kids();
        if let (Some(info), true) = (&info, kids.len() > 2 && kids[1].text_is(".") && kids[2].is("memberName")) {
            let mt = kids[2].node().unwrap().tokens()[0];
            self.check_module_member(info, mt);
        } else {
            // the alias alone or indexed: a module is not a value
            self.module_value(info.as_ref(), root);
        }
    }

    /// A module alias standing alone in a place or a constant expression: a module is not a value (4.5; D-36), as
    /// the typer reports it in an expression.
    fn module_value(&mut self, info: Option<&Info>, tok: &Token) {
        let Some(mi) = info else { return };
        if mi.kind != "module" {
            return;
        }
        let cands = if mi.has_module { self.lib.modules.get(mi.module.as_ref().unwrap()).map(|lm| Self::exported(lm)).unwrap_or_default() } else { vec![] };
        self.rec(
            "E-NAM-001",
            format!("`{}` names the module {}, which is not a value: name a member as `{}.member`", tok.text, mi.path, tok.text),
            "LANG-04-03",
            tok,
            Self::nf(&tok.text, cands),
        );
    }

    /// `alias.member` in a type, a constant expression or a place: the aliased module must declare the member and
    /// export it (D-36; an expression's is the typer's, which knows whether a local shadows the alias).
    fn check_module_member(&mut self, mi: &Info, tok: &Token) {
        if mi.kind != "module" || !mi.has_module {
            return;
        }
        let Some(lm) = self.lib.modules.get(mi.module.as_ref().unwrap()).cloned() else { return };
        match lm.decls.get(&tok.text) {
            None => self.rec("E-NAM-001", format!("{} declares no {}", mi.path, tok.text), "LANG-04-03", tok, Self::nf(&tok.text, Self::exported(&lm))),
            Some(d) if !d.exported => self.rec("E-NAM-004", format!("{} does not export `{}`", mi.path, tok.text), "LANG-04-05", tok, Self::nf(&tok.text, Self::exported(&lm))),
            _ => {}
        }
    }

    fn check_root(&mut self, tok: &Token, scope: usize) -> Option<Info> {
        if let Some(i) = self.lookup(scope, &tok.text) {
            return Some(i.clone());
        }
        if let Some(outer) = self.lambda_outer(scope) {
            if self.lookup(outer, &tok.text).is_some() {
                self.rec(
                    "E-FUN-011",
                    format!("`{}` is not in the lambda's capture list", tok.text),
                    "LANG-08-03",
                    tok,
                    Json::obj([("violation", Json::str("uncaptured name")), ("capture_span", Json::str(&tok.text)), ("use_span", Json::str(&tok.text))]),
                );
                return None;
            }
        }
        let first = tok.text.chars().next().map(|c| c.to_lowercase().to_string()).unwrap_or_default();
        let cands: Vec<String> = self.scopes[self.module_scope].names.keys().filter(|k| k.to_lowercase().starts_with(&first)).take(6).cloned().collect();
        self.rec("E-NAM-001", format!("`{}` is not declared", tok.text), "LANG-04-05", tok, Self::nf(&tok.text, cands));
        if let Some(best) = self.closest(scope, &tok.text) {
            let end = tok.col + tok.text.chars().count();
            let edit = Json::obj([
                ("file", Json::str(&self.file)),
                ("line", Json::Int(tok.line as i64)),
                ("column", Json::Int(tok.col as i64)),
                ("end_line", Json::Int(tok.line as i64)),
                ("end_column", Json::Int(end as i64)),
                ("replacement", Json::str(&best)),
            ]);
            self.records.last_mut().unwrap().correction = Some(Json::obj([("kind", Json::str(format!("rename to `{best}`"))), ("edits", Json::Arr(vec![edit]))]));
        }
        None
    }

    /// The visible name nearest to `name` (at most two edits away, a unique best), for a rename correction.
    fn closest(&self, scope: usize, name: &str) -> Option<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut s = Some(scope);
        while let Some(i) = s {
            for k in self.scopes[i].names.keys() {
                if !seen.contains(k) {
                    seen.push(k.clone());
                }
            }
            s = self.scopes[i].parent;
        }
        let mut best: Vec<(usize, String)> = seen.into_iter().filter(|k| k != name).map(|k| (levenshtein(name, &k), k)).filter(|(d, _)| *d <= 2).collect();
        best.sort();
        match best.as_slice() {
            [(d, k), rest @ ..] if rest.first().map_or(true, |(d2, _)| d2 > d) => Some(k.clone()),
            _ => None,
        }
    }

    fn lambda_outer(&self, scope: usize) -> Option<usize> {
        let mut s = Some(scope);
        while let Some(i) = s {
            if let Some(o) = self.scopes[i].lambda_outer {
                return Some(o);
            }
            s = self.scopes[i].parent;
        }
        None
    }

    fn find_all_skipping<'a>(node: N<'a>, rule: &str, skip: &BTreeSet<&str>) -> Vec<N<'a>> {
        let mut out = Vec::new();
        fn go<'a>(n: N<'a>, root: N<'a>, rule: &str, skip: &BTreeSet<&str>, out: &mut Vec<N<'a>>) {
            if skip.contains(n.name()) && !n.same(root) {
                return;
            }
            if n.name() == rule {
                out.push(n);
            }
            for c in n.kids() {
                if let Some(m) = c.node() {
                    go(m, root, rule, skip, out);
                }
            }
        }
        go(node, node, rule, skip, &mut out);
        out
    }

    fn check_types_in(&mut self, node: N, scope: usize, skip: &BTreeSet<&str>) {
        for tr in Self::find_all_skipping(node, "typeRef", skip) {
            self.check_typeref(tr, scope);
        }
        self.check_origins(node, scope, skip);
    }

    /// One typeRef's own root: `[alias "."] UpperIdent` (its type arguments are typeRefs of their own).
    fn check_typeref(&mut self, tr: N, scope: usize) {
        let ts = tr.direct_tokens();
        if ts.len() >= 2 && ts[0].kind == Kind::Lower && ts[1].text == "." {
            let m = self.lookup(scope, &ts[0].text).cloned();
            match m {
                None => {
                    self.rec("E-NAM-001", format!("module alias `{}` is not imported", ts[0].text), "LANG-04-03", ts[0], Self::nf(&ts[0].text, vec![]));
                }
                Some(mi) => {
                    if let Some(t2) = ts.get(2) {
                        self.check_module_member(&mi, t2);
                    }
                }
            }
            return;
        }
        if let Some(up) = ts.iter().find(|t| t.kind == Kind::Upper) {
            if up.text != "Self" && self.lookup(scope, &up.text).is_none() {
                self.rec("E-NAM-001", format!("type `{}` is not declared", up.text), "LANG-04-05", up, Self::nf(&up.text, vec![]));
            }
        }
    }

    fn check_origins(&mut self, node: N, scope: usize, skip: &BTreeSet<&str>) {
        for org in Self::find_all_skipping(node, "origin", skip) {
            let t = org.tokens()[0];
            if t.kind == Kind::Lower && t.text != "static" && self.lookup(scope, &t.text).is_none() {
                self.rec("E-NAM-001", format!("loan origin `{}` is not a parameter or lifetime", t.text), "LANG-07-06", t, Self::nf(&t.text, vec![]));
            }
        }
    }

    fn interpolation_holes(&mut self, tok: &Token) -> Vec<Arc<Parsed>> {
        let cs: Vec<char> = tok.text.chars().collect();
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
                let src: String = body[i + 2..j.saturating_sub(1).max(i + 2)].iter().collect();
                if let Ok(p) = parse_from(&src, "expr") {
                    out.push(Arc::new(p));
                }
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }

    fn resolve_expr(&mut self, node: C, scope: usize) {
        let node = match node {
            C::T(t) => {
                if t.kind == Kind::Str && t.text.contains("${") {
                    for hole in self.interpolation_holes(t) {
                        let before = self.records.len();
                        self.resolve_expr(C::N(N::new(&hole.root, &hole.toks)), scope);
                        for r in &mut self.records[before..] {
                            r.line = t.line;
                            r.column = t.col;
                            r.correction = None; // its edits are relative to the hole, not the file
                        }
                    }
                }
                return;
            }
            C::N(n) => n,
        };
        let empty = BTreeSet::new();
        match node.name() {
            "lambda" => {
                let ms = self.module_scope;
                let s = self.scope(Some(ms));
                self.scopes[s].lambda_outer = Some(scope);
                // the lambda's own parts, not a nested lambda's (D-34)
                if let Some(cl) = node.child_node("captureList") {
                    for cap in cl.find_all("capture") {
                        let pl = cap.find("place");
                        let t = match pl {
                            Some(pl) => pl.direct_tokens()[0],
                            None => cap.direct_tokens()[0],
                        };
                        if !self.module_level(scope, &t.text) {
                            match pl {
                                Some(pl) => self.resolve_place(pl, scope),
                                None => {
                                    self.check_root(t, scope);
                                }
                            }
                        }
                        if self.module_level(scope, &t.text) {
                            // an entry names a local or parameter; a module-level name is not a capture, and the body
                            // names it directly (8.3, X-230), so it is not bound here (and this is its one record)
                            self.rec(
                                "E-FUN-011",
                                format!("`{}` is a module-level name, not a local or parameter: it is not a capture (8.3)", t.text),
                                "LANG-08-03",
                                t,
                                Json::obj([("violation", Json::str("capture of a module-level name")), ("capture_span", Json::str(&t.text)), ("use_span", Json::str(&t.text))]),
                            );
                            continue;
                        }
                        self.declare(s, &t.text, info("capture"));
                    }
                }
                if let Some(lp) = node.child_node("lambdaParams") {
                    self.declare_params(lp, s);
                }
                if let Some(rt) = node.child_node("type") {
                    self.check_types_in(rt, s, &empty);
                }
                if let Some(body) = node.child_node("funcBody") {
                    let b = body.kid(0).and_then(|c| c.node()).filter(|n| n.name() == "block");
                    if let Some(b) = b {
                        self.resolve_block(b, s);
                    } else if let Some(e) = body.find("expr") {
                        self.resolve_expr(C::N(e), s);
                    }
                }
            }
            "matchExpr" => {
                for c in node.kids() {
                    if c.is("expr") {
                        self.resolve_expr(c, scope);
                    } else if c.is("place") {
                        self.resolve_place(c.node().unwrap(), scope);
                    }
                }
                for arm in node.child_nodes("matchArm") {
                    let s = self.scope(Some(scope));
                    if let Some(pat) = arm.find("pattern") {
                        for b in pattern_bindings(pat) {
                            self.declare_binding(b, s, "pattern");
                        }
                    }
                    for c in arm.kids() {
                        if c.is("expr") {
                            self.resolve_expr(c, s);
                        } else if c.is("block") {
                            self.resolve_block(c.node().unwrap(), s);
                        }
                    }
                }
            }
            "ifExpr" => {
                for c in node.kids() {
                    if c.is("expr") || c.is("ifExpr") {
                        self.resolve_expr(c, scope);
                    } else if c.is("block") {
                        self.resolve_block(c.node().unwrap(), scope);
                    }
                }
            }
            "block" => self.resolve_block(node, scope),
            "outcomeExpr" => {
                let kids = node.kids();
                self.resolve_expr(kids[0], scope);
                let words: Vec<&str> = kids.iter().filter_map(|k| k.tok()).map(|t| t.text.as_str()).collect();
                if words.contains(&"catch") {
                    let s = self.scope(Some(scope));
                    if let Some(ident) = kids.iter().filter_map(|k| k.tok()).find(|t| t.kind == Kind::Lower) {
                        self.declare_binding(ident, s, "catch");
                    }
                    if let Some(b) = node.find("block") {
                        self.resolve_block(b, s);
                    }
                } else {
                    for k in &kids[1..] {
                        self.resolve_expr(*k, scope);
                    }
                }
            }
            "primaryExpr" => {
                let ts = node.direct_tokens();
                if let Some(t0) = ts.first() {
                    if t0.kind == Kind::Lower {
                        self.check_root(t0, scope);
                        return;
                    }
                }
                let tr = node.find("typeRef");
                let first_is_tr = match (tr, node.kid(0)) {
                    (Some(tr), Some(C::N(k0))) => k0.same(tr),
                    _ => false,
                };
                if first_is_tr {
                    let tr = tr.unwrap();
                    let trs = tr.direct_tokens();
                    let up = trs.iter().find(|t| t.kind == Kind::Upper).copied();
                    if trs.len() >= 2 && trs[0].kind == Kind::Lower && trs[1].text == "." {
                        self.check_typeref(tr, scope); // the head only: its type arguments follow, its arguments below
                    } else if let Some(up) = up {
                        let inf = self.lookup(scope, &up.text).cloned();
                        let kids = node.kids();
                        let mut member: Option<&Token> = None;
                        for (i, k) in kids.iter().enumerate() {
                            if k.text_is(".") && i + 1 < kids.len() {
                                member = Some(match kids[i + 1] {
                                    C::T(t) => t,
                                    C::N(n) => n.tokens()[0],
                                });
                                break;
                            }
                        }
                        match inf {
                            None => {
                                if !(member.is_none() && self.enum_variants.contains(&up.text)) {
                                    let first = up.text.chars().next().unwrap().to_string();
                                    let cands: Vec<String> = self.enum_variants.iter().filter(|v| v.starts_with(&first)).take(6).cloned().collect();
                                    self.rec("E-NAM-001", format!("`{}` is not declared", up.text), "LANG-04-05", up, Self::nf(&up.text, cands));
                                }
                            }
                            Some(i) => {
                                if let Some(member) = member {
                                    let has_members = !i.members.is_empty();
                                    let missing = has_members && !i.members.contains(&member.text);
                                    let cond = (missing && (i.kind == "class" || i.kind == "enum") && (i.origin == "prelude" || i.origin == "module")) || (missing && i.origin.starts_with("fortis."));
                                    if cond && !["validate", "fromText", "fromBytes"].contains(&member.text.as_str()) {
                                        let cands: Vec<String> = i.members.iter().take(8).cloned().collect();
                                        self.rec("E-NAM-001", format!("`{}` has no member `{}`", up.text, member.text), "LANG-04-05", member, Self::nf(&format!("{}.{}", up.text, member.text), cands));
                                    }
                                }
                            }
                        }
                    }
                    // type arguments and refinements inside the typeRef, each once
                    for sub in tr.find_all("typeRef") {
                        if !sub.same(tr) {
                            self.check_typeref(sub, scope);
                        }
                    }
                    self.check_origins(tr, scope, &empty);
                    for c in node.kids() {
                        if c.is("argList") {
                            self.resolve_expr(c, scope);
                        }
                    }
                    return;
                }
                self.recurse(node, scope);
            }
            "typeRef" => self.check_types_in(node, scope, &empty),
            "place" => self.resolve_place(node, scope),
            "arg" => {
                for c in node.kids() {
                    if c.is("expr") {
                        self.resolve_expr(c, scope);
                    } else if c.is("place") {
                        self.resolve_place(c.node().unwrap(), scope);
                    }
                }
            }
            "recordLit" => {
                for c in node.kids() {
                    if c.is("expr") || c.is("constExpr") {
                        self.resolve_expr(c, scope);
                    }
                }
            }
            "postfix" => {
                for c in node.kids() {
                    if c.is("expr") || c.is("argList") || c.is("methodCall") {
                        self.resolve_expr(c, scope);
                    }
                }
            }
            "methodCall" => {
                if let Some(al) = node.find("argList") {
                    self.resolve_expr(C::N(al), scope);
                }
            }
            "constTerm" => {
                let ts = node.direct_tokens();
                if let Some(t0) = ts.first() {
                    if t0.kind == Kind::Lower {
                        let qualified = ts.len() > 1 && ts[1].text == ".";
                        let root = self.check_root(t0, scope);
                        if !qualified {
                            self.module_value(root.as_ref(), t0);
                            return;
                        }
                        if let (Some(mi), Some(mn)) = (root, node.child_node("memberName")) {
                            self.check_module_member(&mi, mn.tokens()[0]);
                        }
                    }
                }
                self.recurse(node, scope);
            }
            _ => self.recurse(node, scope),
        }
    }

    fn recurse(&mut self, node: N, scope: usize) {
        for c in node.kids() {
            self.resolve_expr(c, scope);
        }
    }
}

/// The names a pattern binds (names.py `pattern_bindings`).
pub fn pattern_bindings<'a>(pat: N<'a>) -> Vec<&'a Token> {
    let mut out = Vec::new();
    let ts = pat.direct_tokens();
    let words: Vec<&str> = ts.iter().map(|t| t.text.as_str()).collect();
    if let Some(i) = words.iter().position(|w| *w == "as") {
        out.push(ts[i + 1]);
    }
    if words.first() == Some(&"otherwise") && ts.len() >= 3 {
        out.push(ts[2]);
    }
    if ts.len() == 1 && ts[0].kind == Kind::Lower && ts[0].text != "_" {
        out.push(ts[0]);
    }
    for pf in pat.find_all("patternField") {
        if let Some(sub) = pf.find("pattern") {
            out.extend(pattern_bindings(sub));
        }
    }
    for sub in pat.child_nodes("pattern") {
        out.extend(pattern_bindings(sub));
    }
    out
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}
