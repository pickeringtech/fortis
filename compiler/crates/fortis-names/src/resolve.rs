//! Package sets and the import split (Language Reference 4.1-4.6): a port of conformance/tools/resolve.py.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fortis_diag::{Json, Record};
use fortis_syntax::view::N;

use crate::library::CORE_MODULES;
use crate::project::{find_package_dir, ManifestError, Package};

/// 4.6's entry forms: `async` and `try` as needed, and `recovers` when the program discharges a failure it did not report
/// (10.4, 11.3; D-111, the user's decision of 26 Oct 2026), in the grammar's modifier order.
pub const MAIN_FORMS: &[&str] = &["", "try", "async", "async try", "recovers", "try recovers", "async recovers", "async try recovers"];

pub struct PackageSet {
    pub records: Vec<Record>,
    pub root_key: String,
    /// packages in discovery order: key (name, or name@version for a second version) -> package
    pub packages: Vec<(String, Package)>,
    pub identities: Vec<String>,
    pub root_dir: PathBuf,
}

/// An `E-LIM-…` record from a parser's quota error (`E-LIM-NNN: what exceeds the budget of B`).
pub fn limit_record(code: &str, message: &str, file: &str) -> Record {
    let text = message.split_once(": ").map_or(message, |x| x.1).to_string();
    let budget = text.split("the budget of ").nth(1).unwrap_or("").to_string();
    Record::new(code, text.clone(), "PROG-SEC-01", file, 1, 1, Json::obj([("budget", Json::str(&budget)), ("operation", Json::str(text.split(" exceeds").next().unwrap_or(&text)))]))
}

/// The record for a manifest that cannot be loaded: its quota (`E-LIM-003`) or a names error.
pub fn manifest_record(e: &crate::project::ManifestError, file: &str) -> Record {
    if e.0.starts_with("E-LIM-003") {
        return limit_record("E-LIM-003", &e.0, file);
    }
    Record::new("E-NAM-001", e.0.clone(), "LANG-04-03", file, 1, 1, nf("", vec![]))
}

pub fn rel(p: &Path) -> String {
    let root = crate::library::repo_root();
    p.strip_prefix(&root).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string())
}

fn nf(name: &str, cands: Vec<String>) -> Json {
    Json::obj([("name", Json::str(name)), ("candidates", Json::strs(cands))])
}

/// The first of 4.6's `List`, `String` and `Void` bound by a declaration or an import of `m`. The package resolver
/// has no scope origins, so a module-level binding of the name is not the prelude type.
fn entry_type_shadow(m: &crate::project::Module) -> Option<String> {
    let decls = m.decls();
    let imports = m.imports();
    for n in ["List", "String", "Void"] {
        let declared = decls.iter().any(|d| d.name.as_deref() == Some(n));
        let imported = imports.iter().any(|imp| match &imp.members {
            Some(ms) => ms.iter().any(|mm| mm == n),
            None => imp.alias.as_deref().or_else(|| imp.path.last().map(|s| s.as_str())) == Some(n),
        });
        if declared || imported {
            return Some(n.to_string());
        }
    }
    None
}

impl PackageSet {
    pub fn load(root_dir: &Path, search: &Path) -> Result<PackageSet, ManifestError> {
        let root = Package::load(root_dir)?;
        let root_key = root.name.clone();
        let mut ps = PackageSet { records: vec![], root_key: root_key.clone(), packages: vec![], identities: vec![root.identity()], root_dir: root_dir.to_path_buf() };
        ps.packages.push((root_key, root));
        let mut todo = vec![0usize];
        while let Some(i) = todo.pop() {
            let deps = ps.packages[i].1.dependencies.clone();
            let pdir = ps.packages[i].1.dir.clone();
            let pid = ps.packages[i].1.identity();
            for (dep, ver) in deps {
                // a standard package: one of the published compatibility set, at the set's version (PLAT-01-01)
                if crate::compat::is_standard(&dep) {
                    if let Some(r) = crate::compat::check(&dep, &ver, &pdir.join("project.yaml")) {
                        ps.records.push(r);
                    }
                    continue;
                }
                let d = find_package_dir(search, &dep, Some(&ver));
                let d = match d {
                    Some(d) => d,
                    None => {
                        ps.record("E-NAM-001", format!("dependency {dep}@{ver} of {pid} not found"), "LANG-04-03", &pdir.join("project.yaml"), 1, 1, nf(&dep, vec![]));
                        continue;
                    }
                };
                let p = Package::load(&d)?;
                let existing = ps.packages.iter().find(|(k, _)| *k == p.name).map(|(_, q)| q.version.clone());
                let key = if existing.is_some() && existing.as_deref() != Some(p.version.as_str()) { format!("{}@{}", p.name, p.version) } else { p.name.clone() };
                if !ps.packages.iter().any(|(k, _)| *k == key) {
                    let id = p.identity();
                    if !ps.identities.contains(&id) {
                        ps.identities.push(id);
                    }
                    ps.packages.push((key, p));
                    todo.push(ps.packages.len() - 1);
                }
            }
        }
        for (_, p) in ps.packages.iter_mut() {
            p.parse_all();
        }
        // a lockfile pins the resolution: it is verified before anything uses the package set (PROG-ARC-04)
        let lock_path = root_dir.join(crate::lock::FILE);
        if let Ok(text) = std::fs::read_to_string(&lock_path) {
            match crate::lock::Lock::parse(&text) {
                Ok(lock) => {
                    let found = lock.verify(root_dir, search, crate::lock::Pins::InProcess);
                    ps.records.extend(found);
                }
                Err(e) if e.starts_with("E-LIM-004") => ps.records.push(limit_record("E-LIM-004", &e, &rel(&lock_path))),
                Err(e) => ps.record("E-NAM-020", format!("the lockfile cannot be read: {e}"), "PROG-ARC-04", &lock_path, 1, 1, nf("fortis.lock", vec![])),
            }
        }
        Ok(ps)
    }

    pub fn root(&self) -> &Package {
        &self.packages[0].1
    }

    fn record(&mut self, code: &str, msg: String, rule: &str, file: &Path, line: usize, col: usize, fields: Json) {
        self.records.push(Record::new(code, msg, rule, &rel(file), line, col, fields));
    }

    /// Packages the import split may name from `pkg`: itself and its declared dependencies (pinned version).
    pub fn visible_packages(&self, pkg: usize) -> Vec<(String, usize)> {
        let p = &self.packages[pkg].1;
        let mut out = vec![(p.name.clone(), pkg)];
        for (dep, ver) in &p.dependencies {
            if crate::compat::is_standard(dep) {
                continue;
            }
            for (i, (_, q)) in self.packages.iter().enumerate() {
                if q.name == *dep && q.version == *ver {
                    if let Some(e) = out.iter_mut().find(|(n, _)| n == dep) {
                        e.1 = i;
                    } else {
                        out.push((dep.clone(), i));
                    }
                }
            }
        }
        out
    }

    fn visible_get(&self, pkg: usize, name: &str) -> Option<usize> {
        self.visible_packages(pkg).into_iter().find(|(n, _)| n == name).map(|(_, i)| i)
    }

    fn check_prefix_ambiguity(&mut self, pkg: usize) {
        let mut names: Vec<String> = self.visible_packages(pkg).into_iter().map(|(n, _)| n).collect();
        names.push("fortis".into());
        names.sort();
        names.dedup();
        let pdir = self.packages[pkg].1.dir.clone();
        for a in &names {
            for b in &names {
                if b != a && b.starts_with(&format!("{a}.")) {
                    let rest = &b[a.len() + 1..];
                    let mods: Vec<String> = if a == "fortis" {
                        CORE_MODULES.iter().map(|s| s.to_string()).collect()
                    } else {
                        let i = self.visible_get(pkg, a).unwrap();
                        self.packages[i].1.modules.keys().cloned().collect()
                    };
                    if mods.iter().any(|m| m == rest || m.starts_with(&format!("{rest}."))) {
                        self.record(
                            "E-NAM-010",
                            format!("package {b} is also module path {rest} of package {a}: the import split is ambiguous"),
                            "LANG-04-03",
                            &pdir.join("project.yaml"),
                            1,
                            1,
                            nf(b, vec![a.clone(), b.clone()]),
                        );
                    }
                }
            }
        }
    }

    fn split(&self, pkg: usize, segs: &[String]) -> (Option<String>, Vec<String>) {
        let mut visible: Vec<String> = self.visible_packages(pkg).into_iter().map(|(n, _)| n).collect();
        visible.push("fortis".into());
        for i in (1..=segs.len()).rev() {
            let cand = segs[..i].join(".");
            if visible.contains(&cand) {
                return (Some(cand), segs[i..].to_vec());
            }
        }
        (None, segs.to_vec())
    }

    fn check_module(&mut self, pkg: usize, modkey: &str) {
        let (file, decl_names, imps) = {
            let m = &self.packages[pkg].1.modules[modkey];
            let decl_names: Vec<String> = m.decls().iter().filter_map(|d| d.name.clone()).collect();
            let imps: Vec<(Vec<String>, Option<String>, Option<Vec<String>>, usize, usize)> =
                m.imports().iter().map(|i| (i.path.clone(), i.alias.clone(), i.members.clone(), i.token.line, i.token.col)).collect();
            (m.file.clone(), decl_names, imps)
        };
        let mut bound: BTreeMap<String, String> = BTreeMap::new();
        for (path, alias, members, line, col) in imps {
            let (pkg_name, rest) = self.split(pkg, &path);
            let pkg_name = match pkg_name {
                None => {
                    let cands = self.visible_packages(pkg).into_iter().map(|(n, _)| n).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                    self.record("E-NAM-001", format!("import {} names no declared package", path.join(".")), "LANG-04-03", &file, line, col, nf(&path.join("."), cands));
                    continue;
                }
                Some(p) => p,
            };
            let mut target_exports: Option<Vec<String>> = None;
            let modname;
            if pkg_name == "fortis" {
                if rest.len() != 1 || !CORE_MODULES.contains(&rest[0].as_str()) {
                    self.record("E-NAM-001", format!("fortis has no module {}", rest.join(".")), "LANG-04-03", &file, line, col, nf(&rest.join("."), CORE_MODULES.iter().map(|s| s.to_string()).collect()));
                    continue;
                }
                modname = rest[0].clone();
            } else {
                let ti = self.visible_get(pkg, &pkg_name).unwrap();
                let tp = &self.packages[ti].1;
                modname = if !rest.is_empty() { rest.join(".") } else { tp.root.clone().unwrap_or_default() };
                if modname.is_empty() {
                    let cands = tp.modules.keys().cloned().collect();
                    self.record("E-NAM-001", format!("package {pkg_name} has no root module; name a module"), "LANG-04-03", &file, line, col, nf(&pkg_name, cands));
                    continue;
                }
                let target = tp.modules.get(&modname);
                if target.is_none() {
                    let cands = tp.modules.keys().cloned().collect();
                    self.record("E-NAM-001", format!("package {pkg_name} has no module {modname}"), "LANG-04-03", &file, line, col, nf(&modname, cands));
                    continue;
                }
                if ti != pkg && !tp.exports.contains(&modname) {
                    let mut cands = tp.exports.clone();
                    cands.sort();
                    self.record("E-NAM-001", format!("module {modname} of {pkg_name} is not in its exports"), "LANG-04-03", &file, line, col, nf(&modname, cands));
                    continue;
                }
                let t = target.unwrap();
                target_exports = Some(t.decls().iter().filter(|d| d.exported && d.name.is_some()).map(|d| d.name.clone().unwrap()).collect());
            }
            let names: Vec<String> = match &members {
                None => vec![alias.clone().unwrap_or_else(|| path.last().unwrap().clone())],
                Some(ms) => {
                    if let Some(exported) = &target_exports {
                        let mut ex = exported.clone();
                        ex.sort();
                        ex.dedup();
                        for m in ms {
                            if !exported.contains(m) {
                                self.record("E-NAM-001", format!("{pkg_name}.{modname} exports no {m}"), "LANG-04-04", &file, line, col, nf(m, ex.clone()));
                            }
                        }
                    }
                    ms.clone()
                }
            };
            for n in names {
                if bound.contains_key(&n) || decl_names.contains(&n) {
                    // the candidate is the first binding: a later import of the same name does not replace it (LR 4.3)
                    let c = bound.get(&n).cloned().unwrap_or_else(|| "declaration".into());
                    self.record("E-NAM-001", format!("import binds {n} twice or collides with a declaration"), "LANG-04-03", &file, line, col, nf(&n, vec![c]));
                }
                bound.entry(n).or_insert_with(|| path.join("."));
            }
        }
    }

    fn check_entry(&mut self) {
        let (entry, dir) = (self.root().entry.clone(), self.root().dir.clone());
        let entry = match entry {
            Some(e) => e,
            None => return,
        };
        let res = {
            let pkg = self.root();
            match pkg.modules.get(&entry) {
                None => Err((format!("entry module {entry} does not exist"), dir.join("project.yaml"), 1, 1, nf(&entry, pkg.modules.keys().cloned().collect()))),
                Some(m) => {
                    let decls = m.decls();
                    let mains: Vec<_> = decls.iter().filter(|d| d.kind == "func" && d.name.as_deref() == Some("main")).collect();
                    if mains.is_empty() || !mains[0].exported {
                        Err((format!("entry module {entry} exports no main"), m.file.clone(), 1, 1, nf("main", decls.iter().filter_map(|d| d.name.clone()).collect())))
                    } else {
                        let head = mains[0].node.find("funcHead").unwrap();
                        let words = head.find("modifiers").map(|m| m.tokens().iter().map(|t| t.text.clone()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                        let sig = head.find("funcSignature").unwrap();
                        let text = sig.tokens().iter().map(|t| t.text.clone()).collect::<Vec<_>>().join(" ");
                        let form = "export [async] [try] [recovers] func main(args: List[String]) Void";
                        let tok = mains[0].token.unwrap();
                        if MAIN_FORMS.contains(&words.as_str()) && text == "( args : List [ String ] ) Void" {
                            // the token text can match while `List`, `String` or `Void` names a declaration or import
                            // of this module. 4.6's forms use the prelude types (D-51.2)
                            match entry_type_shadow(m) {
                                Some(n) => Err((format!("main's `{n}` is not the prelude {n}, so the signature is not one of the forms of 4.6"), m.file.clone(), tok.line, tok.col, nf("main", vec![form.into()]))),
                                None => Ok(()),
                            }
                        } else {
                            Err((format!("main has signature `{words} func main{text}`, not one of the forms of 4.6"), m.file.clone(), tok.line, tok.col, nf("main", vec![form.into()])))
                        }
                    }
                }
            }
        };
        if let Err((msg, file, line, col, fields)) = res {
            self.record("E-NAM-001", msg, "LANG-04-06", &file, line, col, fields);
        }
    }

    /// The module key of the set's entry module: its root package's `entry` (4.6), or None for a library.
    pub fn entry_key(&self) -> Option<String> {
        let r = self.root();
        r.entry.as_ref().map(|e| format!("{}.{e}", r.name))
    }

    pub fn run(&mut self) -> Vec<Record> {
        let mut errs = Vec::new();
        for (_, p) in &self.packages {
            for m in p.modules.values() {
                if let Some((msg, line, col)) = &m.error {
                    errs.push((msg.clone(), m.file.clone(), *line, *col));
                }
            }
        }
        for (msg, file, line, col) in errs {
            self.record("E-SYN-000", msg, "LANG-03-02", &file, line, col, Json::obj([("expected_tokens", Json::strs(Vec::<String>::new())), ("found_token", Json::str(""))]));
        }
        self.check_prefix_ambiguity(0);
        if !self.records.is_empty() {
            return self.records.clone();
        }
        for i in 0..self.packages.len() {
            let keys: Vec<String> = self.packages[i].1.modules.iter().filter(|(_, m)| m.parsed.is_some()).map(|(k, _)| k.clone()).collect();
            for k in keys {
                self.check_module(i, &k);
            }
        }
        self.check_entry();
        self.records.clone()
    }

    /// `run` in parts, for a host that keeps the set and re-runs one module's part after an edit (the daemon,
    /// SP-12). Each returns the records its part of `run` adds and leaves `records` as it found it. The load's own
    /// records, the syntax errors and the prefix ambiguity come first; when there are any, `run` stops there.
    pub fn opening_records(&mut self) -> Vec<Record> {
        let saved = self.records.clone();
        let mut errs = Vec::new();
        for (_, p) in &self.packages {
            for m in p.modules.values() {
                if let Some((msg, line, col)) = &m.error {
                    errs.push((msg.clone(), m.file.clone(), *line, *col));
                }
            }
        }
        for (msg, file, line, col) in errs {
            self.record("E-SYN-000", msg, "LANG-03-02", &file, line, col, Json::obj([("expected_tokens", Json::strs(Vec::<String>::new())), ("found_token", Json::str(""))]));
        }
        self.check_prefix_ambiguity(0);
        std::mem::replace(&mut self.records, saved)
    }

    /// The import checks of one parsed module, as `run` makes them.
    pub fn module_records(&mut self, pkg: usize, key: &str) -> Vec<Record> {
        let saved = std::mem::take(&mut self.records);
        self.check_module(pkg, key);
        std::mem::replace(&mut self.records, saved)
    }

    /// The entry module's check, as `run` ends.
    pub fn entry_records(&mut self) -> Vec<Record> {
        let saved = std::mem::take(&mut self.records);
        self.check_entry();
        std::mem::replace(&mut self.records, saved)
    }

    pub fn module_tree(&self, pkg: usize, key: &str) -> Option<N<'_>> {
        self.packages[pkg].1.modules.get(key).and_then(|m| m.tree())
    }
}
