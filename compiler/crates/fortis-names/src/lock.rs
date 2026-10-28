//! The lockfile `fortis.lock` (Programme 2.1, 10; PROG-ARC-04; DEL-42).
//!
//! A lockfile pins what a build depends on. It names the root package and records digests of the compiler, the
//! standard library (`library/fortis`, `reference/library` and the native runtime crates), the Rust toolchain
//! (`rustc -vV`), and every package the root resolves to, by identity. When a package has a lockfile, its
//! resolution uses only the lockfile. Every dependency that a manifest names must be pinned there at that version.
//! The package found for an identity must have the pinned digest, and the toolchain the pinned digests. A
//! divergence is refused with a record naming it. `E-NAM-020` is a lockfile that disagrees with the manifests;
//! `E-NAM-021` is a digest that differs from the lockfile. Nothing is fetched: packages come from the search
//! directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fortis_diag::{Json, Record};

use crate::project::{find_package_dir, Package};
use crate::sha256::{sha256, Sha256};

pub const SCHEMA: &str = "fortis.lock/v1";
pub const FILE: &str = "fortis.lock";

/// A lockfile's content.
#[derive(Clone, Debug, PartialEq)]
pub struct Lock {
    pub root: String,
    pub compiler: String,
    pub stdlib: String,
    pub rust: String,
    /// identity (`name@version`) to digest
    pub packages: BTreeMap<String, String>,
}

/// The digest of a directory: every file below it, in path order, as its relative path and its bytes. A lockfile,
/// build output and hidden entries are left out.
pub fn dir_digest(dir: &Path) -> String {
    let mut files = Vec::new();
    collect(dir, dir, &mut files);
    files.sort();
    let mut h = Sha256::new();
    for (rel, path) in files {
        let bytes = std::fs::read(&path).unwrap_or_default();
        h.update(rel.as_bytes());
        h.update(&[0]);
        h.update(&(bytes.len() as u64).to_be_bytes());
        h.update(&bytes);
    }
    h.finish()
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "build" || name == FILE {
            continue;
        }
        if p.is_dir() {
            collect(base, &p, out);
        } else {
            let rel = p.strip_prefix(base).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or(name);
            out.push((rel, p));
        }
    }
}

/// The compiler's digest: the running executable, or, for the daemon, `fortisc` beside it.
pub fn compiler_digest() -> String {
    // the daemon (`fortisd`) answers as the compiler it is built with, `fortisc` beside it, so that its records and
    // the batch check's name one compiler (SP-12b apparatus review 1)
    let exe = std::env::current_exe().ok().map(|p| if p.file_name().is_some_and(|n| n == "fortisd") { p.with_file_name("fortisc") } else { p });
    let exe = exe.and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    format!("fortisc {} sha256:{}", env!("CARGO_PKG_VERSION"), sha256(&exe))
}

/// The standard library's digest: the library sources and the native runtime crates.
pub fn stdlib_digest() -> String {
    let root = crate::library::repo_root();
    let mut h = Sha256::new();
    for d in ["library/fortis", "reference/library", "compiler/crates/fortis-native/src", "compiler/crates/fortis-runtime/src"] {
        h.update(d.as_bytes());
        h.update(dir_digest(&root.join(d)).as_bytes());
    }
    format!("fortis 1.0 sha256:{}", h.finish())
}

/// The Rust toolchain's digest: `rustc -vV`, or `none` when no toolchain is installed.
pub fn rust_digest() -> String {
    match std::process::Command::new("rustc").arg("-vV").output() {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let release = text.lines().find_map(|l| l.strip_prefix("release: ")).unwrap_or("?").to_string();
            format!("rustc {release} sha256:{}", sha256(text.as_bytes()))
        }
        _ => "none".into(),
    }
}

pub const MAX_LOCK_BYTES: usize = 8 << 20;
pub const MAX_LOCK_LINES: usize = 100_000;

/// What a verification compares (DEL-42, DEL-43): the packages only; also the compiler and the standard library,
/// whose digests are computed in this process (what `check` verifies, PROG-FND-02); or also the Rust toolchain,
/// which runs `rustc -vV` (what `build` and `lock --check` verify).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pins {
    Packages,
    InProcess,
    All,
}

impl Lock {
    /// The lockfile of a package resolved from its manifests: every package it reaches, by identity.
    pub fn compute(root_dir: &Path, search: &Path) -> Result<(Lock, Vec<Record>), String> {
        let root = Package::load(root_dir).map_err(|e| e.0)?;
        let mut packages = BTreeMap::new();
        let mut records = Vec::new();
        let mut todo = vec![root.clone_light()];
        let mut seen = vec![root.identity()];
        while let Some(p) = todo.pop() {
            for (dep, ver) in &p.dependencies {
                if crate::compat::is_standard(dep) {
                    continue;
                }
                let id = format!("{dep}@{ver}");
                let Some(d) = find_package_dir(search, dep, Some(ver)) else {
                    records.push(Record::new("E-NAM-001", format!("dependency {id} of {} not found", p.identity()), "LANG-04-03", &crate::resolve::rel(&p.dir.join("project.yaml")), 1, 1, nf(dep, vec![])));
                    continue;
                };
                let q = Package::load(&d).map_err(|e| e.0)?;
                if q.version != *ver {
                    records.push(Record::new("E-NAM-001", format!("dependency {id} of {}: the package found is {}", p.identity(), q.identity()), "LANG-04-03", &crate::resolve::rel(&p.dir.join("project.yaml")), 1, 1, nf(dep, vec![q.identity()])));
                    continue;
                }
                packages.insert(id.clone(), dir_digest(&d));
                if !seen.contains(&id) {
                    seen.push(id);
                    todo.push(q.clone_light());
                }
            }
        }
        Ok((Lock { root: root.identity(), compiler: compiler_digest(), stdlib: stdlib_digest(), rust: rust_digest(), packages }, records))
    }

    pub fn render(&self) -> String {
        let mut s = format!("schema: {SCHEMA}\nroot: {}\ncompiler: {}\nstdlib: {}\nrust: {}\npackages:\n", self.root, self.compiler, self.stdlib, self.rust);
        for (id, d) in &self.packages {
            s.push_str(&format!("  {id}: sha256:{d}\n"));
        }
        s
    }

    pub fn parse(text: &str) -> Result<Lock, String> {
        // the lockfile quotas (PROG-SEC-01)
        if text.len() > MAX_LOCK_BYTES || text.lines().count() > MAX_LOCK_LINES {
            return Err(format!("E-LIM-004: a lockfile of {} bytes and {} lines exceeds the budget of {MAX_LOCK_BYTES} bytes and {MAX_LOCK_LINES} lines", text.len(), text.lines().count()));
        }
        let mut fields: BTreeMap<String, String> = BTreeMap::new();
        let mut packages = BTreeMap::new();
        let mut in_packages = false;
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("  ") {
                if !in_packages {
                    return Err(format!("line {}: an indented line outside `packages:`", n + 1));
                }
                let (id, d) = rest.split_once(':').ok_or(format!("line {}: expected `name@version: sha256:digest`", n + 1))?;
                let d = d.trim().strip_prefix("sha256:").ok_or(format!("line {}: a package digest is `sha256:…`", n + 1))?;
                packages.insert(id.trim().to_string(), d.to_string());
                continue;
            }
            let (k, v) = line.split_once(':').ok_or(format!("line {}: expected `key: value`", n + 1))?;
            in_packages = k.trim() == "packages";
            if !in_packages {
                fields.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        if fields.get("schema").map(String::as_str) != Some(SCHEMA) {
            return Err(format!("not a {SCHEMA} lockfile"));
        }
        let get = |k: &str| fields.get(k).cloned().ok_or(format!("the lockfile has no `{k}`"));
        Ok(Lock { root: get("root")?, compiler: get("compiler")?, stdlib: get("stdlib")?, rust: get("rust")?, packages })
    }

    /// The divergences of this lockfile from the manifests, the packages found and the toolchain, as records.
    /// `toolchain` compares the compiler, standard library and Rust digests too (a check run by a different
    /// compiler build is a divergence; a lockfile written for a fixture may leave them out).
    pub fn verify(&self, root_dir: &Path, search: &Path, pins: Pins) -> Vec<Record> {
        let file = crate::resolve::rel(&root_dir.join(FILE));
        let mut out = Vec::new();
        let diverge = |what: &str, lock: &str, found: &str, out: &mut Vec<Record>| {
            out.push(Record::new("E-NAM-020", format!("the lockfile disagrees with the manifests: {what} is {found}, the lockfile pins {lock}"), "PROG-ARC-04", &file, 1, 1, nf(what, vec![lock.to_string(), found.to_string()])));
        };
        let differ = |what: &str, lock: &str, found: &str, out: &mut Vec<Record>| {
            out.push(Record::new("E-NAM-021", format!("the digest of {what} differs from the lockfile: found {found}, pinned {lock}"), "PROG-ARC-04", &file, 1, 1, nf(what, vec![lock.to_string(), found.to_string()])));
        };
        let root = match Package::load(root_dir) {
            Ok(r) => r,
            Err(e) => {
                diverge("the root manifest", &self.root, &e.0, &mut out);
                return out;
            }
        };
        if root.identity() != self.root {
            diverge("the root package", &self.root, &root.identity(), &mut out);
        }
        if pins != Pins::Packages {
            let mut toolchain = vec![("the compiler", &self.compiler, compiler_digest()), ("the standard library", &self.stdlib, stdlib_digest())];
            if pins == Pins::All {
                toolchain.push(("the Rust toolchain", &self.rust, rust_digest()));
            }
            for (what, lock, found) in toolchain {
                if *lock != found {
                    differ(what, lock, &found, &mut out);
                }
            }
        }
        // every dependency a reached manifest names is pinned at that version; the package found has the digest
        let mut todo = vec![root.clone_light()];
        let mut reached: Vec<String> = Vec::new();
        while let Some(p) = todo.pop() {
            for (dep, ver) in &p.dependencies {
                if crate::compat::is_standard(dep) {
                    continue;
                }
                let id = format!("{dep}@{ver}");
                let Some(pinned) = self.packages.get(&id) else {
                    let other = self.packages.keys().find(|k| k.split('@').next() == Some(dep.as_str())).cloned().unwrap_or_else(|| "nothing".into());
                    diverge(&format!("dependency {dep} of {}", p.identity()), &other, &id, &mut out);
                    continue;
                };
                if reached.contains(&id) {
                    continue;
                }
                reached.push(id.clone());
                let Some(d) = find_package_dir(search, dep, Some(ver)) else {
                    out.push(Record::new("E-NAM-001", format!("the lockfile pins {id}, which is not in the search directory"), "LANG-04-03", &file, 1, 1, nf(dep, vec![])));
                    continue;
                };
                let found = dir_digest(&d);
                if found != *pinned {
                    differ(&format!("package {id}"), &format!("sha256:{pinned}"), &format!("sha256:{found}"), &mut out);
                }
                if let Ok(q) = Package::load(&d) {
                    todo.push(q.clone_light());
                }
            }
        }
        for id in self.packages.keys() {
            if !reached.contains(id) {
                diverge(&format!("package {id}"), id, "not a dependency of any manifest", &mut out);
            }
        }
        out
    }
}

fn nf(name: &str, cands: Vec<String>) -> Json {
    Json::obj([("name", Json::str(name)), ("candidates", Json::strs(cands))])
}

impl Package {
    /// The manifest facts resolution needs, without the modules.
    fn clone_light(&self) -> Package {
        Package { dir: self.dir.clone(), name: self.name.clone(), version: self.version.clone(), exports: vec![], entry: None, root: None, dependencies: self.dependencies.clone(), modules: BTreeMap::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_parses() {
        let mut packages = BTreeMap::new();
        packages.insert("a.b@1.0.0".to_string(), "00".repeat(32));
        let l = Lock { root: "app@0.1.0".into(), compiler: "fortisc 0.1.0 sha256:aa".into(), stdlib: "fortis 1.0 sha256:bb".into(), rust: "none".into(), packages };
        assert_eq!(Lock::parse(&l.render()).unwrap(), l);
        assert!(Lock::parse("schema: other\n").is_err());
    }
}
