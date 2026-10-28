//! The typed IR builder: lowers a module the checker accepted, using the checker's own tables (declarations, the
//! type of every expression node of its final pass, closure environments, effect summaries), into `crate::model`.
//!
//! The builder walks the syntax tree in the definitional interpreter's evaluation order (interpreter/eval.py) and
//! makes every implicit step explicit: name resolution, method dispatch, call modes, argument binding with
//! defaults, optional coercions, closure environments and outcome handling. It never re-decides a static
//! question: anything it cannot resolve from the checker's tables is an error naming the construct.

mod aspect;
mod expr;
mod fb;
mod pat;
mod stmt;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use fortis_syntax::lexer::{Kind, Token};
use fortis_syntax::view::{declarations, N};
use fortis_types::decls::{named_of_decl, DeclRef, Sig};
use fortis_types::effects::Summary;
use fortis_types::typer::Typer;
use fortis_types::types::*;

use crate::model::*;
pub use fb::Fb;

/// Which core-library members have Fortis bodies (library/fortis/*.ft, DEL-26..28); the rest are intrinsics.
#[derive(Clone, Debug, Default)]
pub struct LibIndex {
    /// labels with a body, each with its module: ("method List.map", "fortis.prelude")
    pub bodies: HashMap<String, String>,
    /// how this program's colliding modules and types are named (X-136); empty for most programs
    pub qualify: crate::names::Qualify,
}

pub const LIBRARY_MODULES: &[&str] = &["prelude", "text", "collections", "numeric", "time", "money", "host"];

/// A module of a kit package (fortis.data, fortis.http, ...) rather than the core library.
pub fn is_kit_module(m: &str) -> bool {
    m.starts_with("fortis.") && !LIBRARY_MODULES.iter().any(|x| m == format!("fortis.{x}"))
}

impl LibIndex {
    pub fn load(root: &Path) -> LibIndex {
        let mut bodies = HashMap::new();
        for m in LIBRARY_MODULES {
            let path = root.join("library").join("fortis").join(format!("{m}.ft"));
            let src = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let parsed = match fortis_syntax::parse_module(&src) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let root_n = N::new(&parsed.root, &parsed.toks);
            let module = format!("fortis.{m}");
            for d in declarations(root_n) {
                let name = d.name.clone().unwrap_or_default();
                match d.kind {
                    "func" => {
                        if d.node.find("funcBody").is_some() {
                            bodies.insert(format!("function {name}"), module.clone());
                        }
                    }
                    "class" | "enum" => {
                        for mem in d.node.kids().into_iter().filter_map(|c| c.node()) {
                            if mem.name() != "classMember" && mem.name() != "enumMember" {
                                continue;
                            }
                            if let (Some(h), Some(_)) = (mem.find("methodHead"), mem.find("funcBody")) {
                                let mn = h.find("memberName").map(|x| x.tokens()[0].text.clone()).unwrap_or_default();
                                bodies.insert(format!("method {name}.{mn}"), module.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        LibIndex { bodies, qualify: Default::default() }
    }
}

/// A builder error: the construct the typed IR does not (yet) represent, with its line.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildError {
    pub line: u32,
    pub message: String,
}

pub type BResult<T> = Result<T, BuildError>;

pub fn err<T>(line: u32, message: impl Into<String>) -> BResult<T> {
    Err(BuildError { line, message: message.into() })
}

pub struct Builder<'t, 'a> {
    pub ty: &'t mut Typer<'a>,
    pub types: HashMap<usize, Type>,
    pub module: String,
    /// the module's name in the IR: `module`, or `module@version` when its package is linked at two versions
    pub ir_module: String,
    pub lib: &'t LibIndex,
    pub prog: Program,
    /// the function builders being filled: the last is current; a lambda pushes one
    pub fbs: Vec<Fb>,
    /// module-level names: constants, by name, to their qualified name
    pub consts: HashSet<String>,
    pub errors: Vec<BuildError>,
    /// the value under construction is moved into its consumer, so it is not itself a statement temporary (7.9)
    pub consuming: bool,
}

pub fn line_of(n: N) -> u32 {
    n.tokens().first().map_or(0, |t| t.line as u32)
}

pub fn tok_line(t: &Token) -> u32 {
    t.line as u32
}

impl<'t, 'a> Builder<'t, 'a> {
    pub fn new(ty: &'t mut Typer<'a>, module: &str, lib: &'t LibIndex) -> Builder<'t, 'a> {
        let types = ty.node_types.take().unwrap_or_default();
        let ir_module = lib.qualify.module(module, &ty.dc.package);
        Builder { ty, types, module: module.to_string(), ir_module, lib, prog: Program::default(), fbs: vec![], consts: HashSet::new(), errors: vec![], consuming: false }
    }

    /// The type the checker gave a node in its final pass.
    pub fn node_ty(&self, n: N) -> Type {
        self.types.get(&n.id()).cloned().unwrap_or_else(|| unknown("untyped"))
    }

    pub fn qualified(&self, module: &str, label: &str) -> String {
        format!("{module}::{label}")
    }

    /// The IR name of the module a declaration belongs to (X-136).
    pub fn ir_mod(&self, d: &fortis_types::decls::Decl<'a>) -> String {
        self.lib.qualify.module(&d.module, &d.package)
    }

    /// The IR name of a type declared by the package with this identity (X-136).
    pub fn tname(&self, name: &str, package: &str) -> String {
        self.lib.qualify.type_name(name, package)
    }

    /// A standard enum a user type shadows (`enum Currency` beside `money.Currency`) is `Name@module`
    /// (`Currency@fortis.money`). Native then emits a different Rust enum from the program's. Every other enum
    /// keeps the name `tname` gives it (D-142).
    pub fn enum_ir_name(&self, d: &DeclRef<'a>) -> String {
        if let Some(n) = self.shadow_enum_name(d) {
            return n;
        }
        self.tname(&d.name, &d.type_identity())
    }

    fn shadow_enum_name(&self, d: &DeclRef<'a>) -> Option<String> {
        if !fortis_types::decls::is_standard(&d.module) {
            return None;
        }
        let bare = self.ty.dc.table.get(&d.name)?;
        if bare.module == d.module || fortis_types::decls::is_standard(&bare.module) {
            return None;
        }
        Some(format!("{}@{}", d.name, d.module))
    }

    /// The root nominal of `t` spelled as `ir_name`, when that name disambiguates a shadowed standard enum.
    /// Type arguments stay as written.
    pub fn rename_shadow(&self, t: &Type, ir_name: &str) -> Type {
        let Some((bare, _)) = ir_name.split_once('@') else { return t.clone() };
        match t {
            Type::Named(n) if n.name == bare => {
                let mut n = n.clone();
                n.name = ir_name.to_string();
                Type::Named(n)
            }
            Type::Refined { base, lo, hi, kind } if base.name == bare => {
                let mut base = base.clone();
                base.name = ir_name.to_string();
                Type::Refined { base, lo: *lo, hi: *hi, kind: kind.clone() }
            }
            other => other.clone(),
        }
    }

    /// `fn(...) T` whose result is a shadowed standard enum, with that result disambiguated.
    pub fn rename_fn_ret(&self, t: &Type, ir_name: &str) -> Type {
        match t {
            Type::Fn { params, ret, kind, modifiers, origin, dynamic, identity } => Type::Fn {
                params: params.clone(),
                ret: Box::new(self.rename_shadow(ret, ir_name)),
                kind: kind.clone(),
                modifiers: modifiers.clone(),
                origin: origin.clone(),
                dynamic: *dynamic,
                identity: identity.clone(),
            },
            other => other.clone(),
        }
    }

    /// The qualified function a declaration's member resolves to, or None when it is an intrinsic.
    pub fn member_function(&self, d: &DeclRef<'a>, member: &str, sig: &Sig<'a>) -> Option<String> {
        let label = format!("method {}.{member}", d.name);
        if d.module == self.module {
            return if sig.body.is_some() { Some(self.qualified(&self.ir_module, &label)) } else { None };
        }
        // a core-library member's body lives where the library index says; another package's where it is declared
        let lib = if d.module.starts_with("fortis.") { self.lib.bodies.get(&label).map(|m| self.qualified(m, &label)) } else { None };
        lib.or_else(|| if sig.body.is_some() { Some(self.qualified(&self.ir_mod(d), &label)) } else { None })
    }

    /// The qualified free function a declaration names, or None for an intrinsic.
    pub fn free_function(&self, d: &DeclRef<'a>) -> Option<String> {
        let label = format!("function {}", d.name.rsplit('.').next().unwrap_or(&d.name));
        if d.module == self.module {
            return d.sig.as_ref().and_then(|s| s.body).map(|_| self.qualified(&self.ir_module, &label));
        }
        let lib = if d.module.starts_with("fortis.") { self.lib.bodies.get(&label).map(|m| self.qualified(m, &label)) } else { None };
        lib.or_else(|| d.sig.as_ref().and_then(|s| s.body).map(|_| self.qualified(&self.ir_mod(d), &label)))
    }

    pub fn effects_of(&self, label: &str) -> Effects {
        let s: Option<&Summary> = self.ty.summaries.get(label);
        s.map(|s| Effects { effects: s.effects.clone(), footprint: s.footprint.iter().cloned().collect(), authorities: s.authorities.iter().cloned().collect() }).unwrap_or_default()
    }

    // ---- the module --------------------------------------------------------------------------------------------

    pub fn module(&mut self, root: N<'a>) {
        let decls: Vec<DeclRef<'a>> = self.ty.dc.table.decls.values().filter(|d| d.module == self.module).cloned().collect();
        for d in &decls {
            if d.kind == "const" {
                self.consts.insert(d.name.clone());
            }
        }
        for d in &decls {
            if let Err(e) = self.declaration(d) {
                self.errors.push(e);
            }
        }
        // tests and properties are declarations of the tree, not of the table
        for d in declarations(root) {
            if d.kind == "test" || d.kind == "property" {
                // a test the builder cannot lower is recorded, not fatal: the rest of the program still runs
                let depth = self.fbs.len();
                if let Err(e) = self.test_decl(d.node, d.kind, d.token) {
                    self.fbs.truncate(depth);
                    let title = d.token.map(|t| t.text.trim_matches('"').to_string()).unwrap_or_default();
                    self.prog.unbuilt_tests.push((title, format!("line {}: {}", e.line, e.message)));
                }
            }
        }
        if let Some(f) = self.prog.functions.iter().find(|f| f.label == "function main" && f.module == self.ir_module) {
            self.prog.entry = Some(f.name.clone());
        }
    }

    fn declaration(&mut self, d: &DeclRef<'a>) -> BResult<()> {
        match d.kind.as_str() {
            "func" => {
                let sig = d.sig.clone().ok_or_else(|| BuildError { line: 0, message: format!("function {} has no signature", d.name) })?;
                let label = format!("function {}", d.name);
                self.function(&sig, &label, None, d.aspects.clone())?;
            }
            "class" | "enum" => {
                self.type_def(d)?;
                let self_t = named_of_decl(d, d.type_params.iter().map(|p| Type::TypeParam(p.clone())).collect());
                for (mname, sig) in d.methods.iter() {
                    if sig.body.is_none() {
                        continue;
                    }
                    let label = format!("method {}.{mname}", d.name);
                    self.function(sig, &label, Some(self_t.clone()), vec![])?;
                }
                if d.has_cleanup {
                    self.cleanup_fn(d, &self_t)?;
                }
                // mixin methods become methods of the class (Appendix C.1: generated inherent methods)
                for mx in d.mixins.clone() {
                    if let Some(md) = self.ty.dc.table.get(&mx.name).cloned() {
                        for (mname, sig) in md.methods.iter() {
                            if sig.body.is_none() || d.methods.get(mname).map_or(false, |s| s.body.is_some()) {
                                continue;
                            }
                            let label = format!("method {}.{mname}", d.name);
                            self.function(sig, &label, Some(self_t.clone()), vec![])?;
                        }
                    }
                }
            }
            "type" | "error" | "interface" | "observation" | "mixin" => self.type_def(d)?,
            "const" => self.const_def(d)?,
            _ => {}
        }
        Ok(())
    }

    fn const_def(&mut self, d: &DeclRef<'a>) -> BResult<()> {
        let node = match d.node {
            Some(n) => n,
            None => return Ok(()),
        };
        let ce = match node.child_node("constExpr") {
            Some(c) => c,
            None => return Ok(()),
        };
        let declared = self.ty.inferred_const(&d.module, &d.name).filter(known).or_else(|| d.alias.clone().filter(known)).or_else(|| Some(self.node_ty(ce)).filter(known));
        self.fbs.push(Fb::new(&format!("const {}", d.name), &self.ir_module, void(), false));
        let value = self.const_expr(ce);
        self.fbs.pop();
        let value = value?;
        let ty = declared.unwrap_or_else(|| fortis_types::literals::concrete(&value.ty));
        self.prog.consts.push(ConstDef { name: d.name.clone(), module: self.ir_module.clone(), ty, value });
        Ok(())
    }

    /// A constant expression (a default, a constant's value, a refinement bound): evaluated without locals.
    pub fn const_expr(&mut self, ce: N<'a>) -> BResult<Expr> {
        self.expr(ce)
    }

    fn type_def(&mut self, d: &DeclRef<'a>) -> BResult<()> {
        let mut methods: Vec<(String, String)> = Vec::new();
        for (mname, sig) in d.methods.iter() {
            if let Some(f) = self.member_function(d, mname, sig) {
                methods.push((mname.clone(), f));
            }
        }
        for mx in &d.mixins {
            if let Some(md) = self.ty.dc.table.get(&mx.name) {
                for (mname, sig) in md.methods.iter() {
                    if sig.body.is_some() && !methods.iter().any(|(m, _)| m == mname) {
                        methods.push((mname.clone(), self.qualified(&self.ir_module, &format!("method {}.{mname}", d.name))));
                    }
                }
            }
        }
        let self_t = named_of_decl(d, vec![]);
        let kind = match d.kind.as_str() {
            "class" => {
                // class constants (`const step = 1` in a class body) are constants named `Class.step`
                if let Some(node) = d.node {
                    for m in node.kids().into_iter().filter_map(|c| c.node()) {
                        if let Some(cd) = m.find("constDecl") {
                            let cname = cd.direct_tokens().iter().find(|t| t.kind == Kind::Lower).map(|t| t.text.clone()).unwrap_or_default();
                            if let Some(ce) = cd.child_node("constExpr") {
                                let declared = d.constants.get(&cname).cloned().filter(known);
                                self.fbs.push(Fb::new(&format!("const {}.{cname}", d.name), &self.ir_module, declared.clone().unwrap_or_else(|| unknown("")), false));
                                let v = self.const_expr(ce);
                                self.fbs.pop();
                                let v = v?;
                                let cty = declared.unwrap_or_else(|| fortis_types::literals::concrete(&v.ty));
                                self.prog.consts.push(ConstDef { name: format!("{}.{cname}", d.name), module: self.ir_module.clone(), ty: cty, value: v });
                            }
                        }
                    }
                }
                let mut fields = Vec::new();
                for (fname, (ft, exported, _)) in d.fields.iter() {
                    let default = match self.field_default(d, fname) {
                        Some(ce) => {
                            self.fbs.push(Fb::new(&format!("default {}.{fname}", d.name), &self.ir_module, ft.clone(), false));
                            let v = self.const_expr(ce);
                            self.fbs.pop();
                            Some(self.coerce(v?, ft))
                        }
                        None => None,
                    };
                    fields.push(FieldDef { name: fname.clone(), ty: ft.clone(), exported: *exported, default });
                }
                TypeKind::Class { fields }
            }
            "enum" => TypeKind::Enum { variants: d.variants.iter().map(|(v, fs)| VariantDef { name: v.clone(), fields: fs.clone() }).collect() },
            "type" => TypeKind::Newtype { repr: d.alias.clone().unwrap_or_else(|| unknown("newtype")) },
            "error" => TypeKind::Error {
                code: d.code.clone(),
                payload: d.payload.iter().map(|(n, t)| FieldDef { name: n.clone(), ty: t.clone(), exported: true, default: None }).collect(),
                message: self.error_message(d),
            },
            "interface" => TypeKind::Interface { methods: d.methods.iter().map(|(m, s)| (m.clone(), s.modifiers.clone())).collect() },
            "mixin" => TypeKind::Mixin {
                provides: d.methods.iter().filter(|(_, s)| s.body.is_some()).map(|(m, _)| m.clone()).collect(),
                requires: d.methods.iter().filter(|(_, s)| s.body.is_none()).map(|(m, _)| m.trim_start_matches("requires:").to_string()).collect(),
            },
            "observation" => TypeKind::Observation {
                fields: d.obs_fields.iter().map(|(n, (t, req))| (n.clone(), t.clone(), req.clone())).collect(),
                version: d.obs_items.get("version").and_then(|v| v.parse().ok()).unwrap_or(1),
            },
            _ => return Ok(()),
        };
        let cleanup = if d.has_cleanup { Some(self.qualified(&self.ir_module, &format!("cleanup {}", d.name))) } else { None };
        self.prog.types.push(TypeDef {
            name: self.tname(&d.name, &d.type_identity()),
            module: self.ir_module.clone(),
            type_params: d.type_params.clone(),
            kind,
            trivial: d.trivial,
            copyable: self.ty.dc.is_copyable(&self_t),
            implements: d.implements.iter().map(|i| self.tname(&i.name, &i.package)).collect(),
            mixins: d.mixins.iter().map(|m| self.tname(&m.name, &m.package)).collect(),
            lifetimes: d.lifetime.iter().cloned().collect(),
            cleanup,
            methods,
        });
        Ok(())
    }

    fn error_message(&self, d: &DeclRef<'a>) -> String {
        d.node
            .and_then(|n| n.tokens().into_iter().skip_while(|t| t.text != "message").nth(2).map(|t| t.text.clone()))
            .map(|t| if t.len() >= 2 { t[1..t.len() - 1].to_string() } else { t })
            .unwrap_or_default()
    }

    fn field_default(&self, d: &DeclRef<'a>, fname: &str) -> Option<N<'a>> {
        let node = d.node?;
        for m in node.kids().into_iter().filter_map(|c| c.node()) {
            if let Some(fd) = m.find("fieldDecl") {
                if fd.direct_tokens().first().map_or(false, |t| t.text == fname) {
                    return fd.child_node("constExpr");
                }
            }
        }
        None
    }

    fn cleanup_fn(&mut self, d: &DeclRef<'a>, self_t: &Type) -> BResult<()> {
        let node = d.node.unwrap();
        let cl = node.kids().into_iter().filter_map(|c| c.node()).filter(|m| m.name() == "classMember").find_map(|m| m.find("cleanupDecl"));
        let cl = match cl {
            Some(c) => c,
            None => return Ok(()),
        };
        let label = format!("cleanup {}", d.name);
        let mut fb = Fb::new(&label, &self.ir_module, void(), false);
        let sl = fb.add_local("self", self_t.clone(), true, LocalKind::SelfParam);
        fb.self_local = Some(sl);
        fb.params.push(Param { local: sl, name: "self".into(), mode: Mode::Editable, default: None });
        self.fbs.push(fb);
        let body = self.block(cl.find("block").unwrap());
        let fb = self.fbs.pop().unwrap();
        let body = body?;
        let effects = self.effects_of(&label);
        self.prog.functions.push(fb.finish(line_of(cl), Some(body), vec![], effects, None, None));
        Ok(())
    }

    fn test_decl(&mut self, node: N<'a>, kind: &str, tok: Option<&Token>) -> BResult<()> {
        let tname = tok.map(|t| t.text.clone()).unwrap_or_default();
        let label = format!("test {tname}");
        let mut fb = Fb::new(&label, &self.ir_module, void(), true);
        fb.push_scope();
        if kind == "property" {
            if let Some(pl) = node.find("paramList") {
                for p in pl.child_nodes("param") {
                    let pts = p.direct_tokens();
                    if let (Some(pn), Some(pt)) = (pts.first(), p.find("paramType")) {
                        let t = self.ty.dc.type_of(pt.find("type"));
                        let l = fb.add_local(&pn.text, t, false, LocalKind::Param);
                        fb.params.push(Param { local: l, name: pn.text.clone(), mode: Mode::Owned, default: None });
                        fb.bind(&pn.text, l);
                    }
                }
            }
        }
        let words: Vec<&Token> = node.direct_tokens();
        let row = words.iter().position(|t| t.text == "as").map(|i| words[i + 1].text.clone());
        let generators: Vec<(String, fortis_types::types::Type)> = fb.params.iter().map(|p| (p.name.clone(), fb.locals[p.local as usize].ty.clone())).collect();
        if let Some(r) = &row {
            let l = fb.add_local(r, unknown("test row"), false, LocalKind::Param);
            fb.params.push(Param { local: l, name: r.clone(), mode: Mode::Owned, default: None });
            fb.bind(r, l);
        }
        self.fbs.push(fb);
        // the rows of a table test: constant record literals, evaluated when the test runs
        let mut cases = Vec::new();
        for cr in node.child_nodes("caseRow") {
            let cname = cr.tokens()[0].text.trim_matches('"').to_string();
            match cr.child_node("recordLit") {
                Some(rl) => match self.expr(rl) {
                    Ok(v) => cases.push((cname, v)),
                    Err(e) => {
                        self.fbs.pop();
                        return Err(e);
                    }
                },
                None => {}
            }
        }
        // a table test's row parameter has the type of its rows
        if let (Some(r), Some((_, first))) = (&row, cases.first()) {
            let fb = self.fbs.last_mut().unwrap();
            if let Some(p) = fb.params.iter().find(|p| &p.name == r) {
                let l = p.local as usize;
                fb.locals[l].ty = first.ty.clone();
            }
        }
        let blk = *node.child_nodes("block").last().unwrap();
        let body = self.block(blk);
        let fb = self.fbs.pop().unwrap();
        let body = body?;
        let effects = self.effects_of(&label);
        let name = fb.name.clone();
        self.prog.functions.push(fb.finish(line_of(node), Some(body), vec!["try".into()], effects, None, None));
        let title = tname.trim_matches('"').to_string();
        self.prog.tests.push(TestDef { name: tname, title, kind: kind.into(), function: name, cases, generators: if kind == "property" { generators } else { vec![] } });
        Ok(())
    }

    /// A function, method or associated function with a body; with applied aspects (other than `@observe`), the
    /// woven function and its parts (aspect.rs).
    pub fn function(&mut self, sig: &Sig<'a>, label: &str, self_t: Option<Type>, aspects: Vec<N<'a>>) -> BResult<()> {
        let woven = aspects.iter().any(|a| a.direct_tokens().get(1).map_or(false, |t| t.text != "observe"));
        if woven && sig.body.is_some() {
            return self.woven_function(sig, label, self_t, aspects);
        }
        self.function_plain(sig, label, self_t, aspects)
    }

    pub fn function_plain(&mut self, sig: &Sig<'a>, label: &str, self_t: Option<Type>, aspects: Vec<N<'a>>) -> BResult<()> {
        let body = match sig.body {
            Some(b) => b,
            None => return Ok(()),
        };
        let is_try = sig.has_mod("try");
        let mut fb = Fb::new(label, &self.ir_module, sig.ret.clone(), is_try);
        if let (Some(st), Some(recv)) = (&self_t, &sig.receiver) {
            let mode = match recv.as_str() {
                "editable self" => Mode::Editable,
                "owned self" => Mode::Owned,
                _ => Mode::Read,
            };
            let sl = fb.add_local("self", st.clone(), mode != Mode::Read, LocalKind::SelfParam);
            fb.self_local = Some(sl);
            fb.params.push(Param { local: sl, name: "self".into(), mode, default: None });
        }
        let defaults = self.param_defaults(sig);
        for (pn, pt, pm) in &sig.params {
            let mode = match pm.trim_start_matches("...") {
                "editable" => Mode::Editable,
                "owned" => Mode::Owned,
                _ => Mode::Read,
            };
            let l = fb.add_local(pn, pt.clone(), mode == Mode::Editable, LocalKind::Param);
            fb.bind(pn, l);
            fb.params.push(Param { local: l, name: pn.clone(), mode, default: None });
        }
        self.fbs.push(fb);
        let mut defs = Vec::new();
        for (pn, ce) in defaults {
            let pt = sig.params.iter().find(|p| p.0 == pn).map(|p| p.1.clone()).unwrap_or_else(|| unknown(""));
            let v = self.const_expr(ce);
            match v {
                Ok(v) => defs.push((pn, self.coerce(v, &pt))),
                Err(e) => {
                    self.fbs.pop();
                    return Err(e);
                }
            }
        }
        for (pn, v) in defs {
            if let Some(p) = self.fbs.last_mut().unwrap().params.iter_mut().find(|p| p.name == pn) {
                p.default = Some(v);
            }
        }
        let built = self.func_body(body);
        let fb = self.fbs.pop().unwrap();
        let built = built?;
        // a woven body's summary is its target's (the checker summarises the elaborated function)
        let effects = self.effects_of(label.split('#').next().unwrap_or(label));
        let observe = self.observe_schema(&aspects);
        let line = sig.node.map(line_of).unwrap_or(0);
        self.prog.functions.push(fb.finish(line, Some(built), sig.modifiers.clone(), effects, observe, None));
        Ok(())
    }

    fn param_defaults(&self, sig: &Sig<'a>) -> Vec<(String, N<'a>)> {
        let mut out = Vec::new();
        if let Some(head) = sig.node {
            if let Some(pl) = head.find("paramList") {
                for p in pl.child_nodes("param") {
                    if let (Some(pn), Some(ce)) = (p.direct_tokens().first(), p.child_node("constExpr")) {
                        out.push((pn.text.clone(), ce));
                    }
                }
            }
        }
        out
    }

    /// `@observe(Schema)` among a function's aspects names its observation schema (13).
    fn observe_schema(&self, aspects: &[N<'a>]) -> Option<String> {
        for a in aspects {
            let ts = a.direct_tokens();
            if ts.get(1).map_or(false, |t| t.text == "observe") {
                if let Some(al) = a.find("argList") {
                    let schema = al.tokens().into_iter().find(|t| t.kind == Kind::Upper).map(|t| t.text.clone())?;
                    return Some(self.ty.dc.table.get(&schema).map(|d| self.tname(&d.name, &d.type_identity())).unwrap_or(schema));
                }
            }
        }
        None
    }

    /// A function body: a block, or `=> expr` (a return of the expression).
    pub fn func_body(&mut self, body: N<'a>) -> BResult<Block> {
        if let Some(b) = body.child_node("block") {
            return self.block(b);
        }
        let e = body.child_node("expr").ok_or_else(|| BuildError { line: line_of(body), message: "a function body without a block or expression".into() })?;
        let line = line_of(e);
        let ret = self.fbs.last().unwrap().ret.clone();
        let v = self.expr(e)?;
        let v = self.coerce(v, &ret);
        Ok(Block { stmts: vec![Stmt { line, temps: vec![], kind: StmtKind::Return(Some(v)) }], locals: vec![], line, defers: vec![] })
    }
}

/// Build the typed IR of one checked module. The typer must have run with `record_types` set.
pub fn build_module<'a>(ty: &mut Typer<'a>, root: N<'a>, module: &str, lib: &LibIndex) -> Result<Program, Vec<BuildError>> {
    let mut b = Builder::new(ty, module, lib);
    b.module(root);
    if b.errors.is_empty() {
        Ok(b.prog)
    } else {
        Err(b.errors)
    }
}

#[allow(dead_code)]
fn _unused(_: BTreeMap<String, String>) {}
