//! The reference library (reference/library) and the per-package libraries of a package set: a port of the
//! `Library` and `LibraryModule` classes of conformance/tools/names.py.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use fortis_syntax::view::{declarations, N};
use fortis_syntax::{parse_module, Parsed};

pub const CORE_MODULES: &[&str] = &["prelude", "text", "collections", "numeric", "time", "money", "host"];

/// The repository root: FORTIS_ROOT, else the directory three levels above this crate.
pub fn repo_root() -> PathBuf {
    if let Ok(r) = std::env::var("FORTIS_ROOT") {
        return PathBuf::from(r);
    }
    // the toolchain root holds the standard library: found from this executable's location (it lives in
    // compiler/target/release/ or debug/), then from the working directory; never a path fixed at compile time, so the
    // compiler built in two directories is the same executable (PROG-NFR-22)
    let holds_library = |d: &Path| d.join("library").join("fortis").join("prelude.ft").is_file();
    let from = |start: Option<PathBuf>| start.and_then(|s| s.ancestors().find(|d| holds_library(d)).map(|d| d.to_path_buf()));
    from(std::env::current_exe().ok().and_then(|e| e.canonicalize().ok())).or_else(|| from(std::env::current_dir().ok())).unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Clone, Debug)]
pub struct LibDecl {
    pub kind: &'static str,
    pub exported: bool,
    pub members: BTreeSet<String>,
}

pub struct LibraryModule {
    pub path: String,
    pub package: String,
    pub parsed: Arc<Parsed>,
    pub decls: BTreeMap<String, LibDecl>,
    /// the package library a package module's signatures are read in (its own imports and dependency versions)
    pub library: Option<usize>,
}

impl LibraryModule {
    pub fn new(path: &str, package: &str, parsed: Arc<Parsed>, library: Option<usize>) -> LibraryModule {
        let mut decls = BTreeMap::new();
        {
            let root = N::new(&parsed.root, &parsed.toks);
            for d in declarations(root) {
                if let Some(name) = &d.name {
                    decls.insert(name.clone(), LibDecl { kind: d.kind, exported: d.exported, members: members_of(d.node) });
                }
            }
        }
        LibraryModule { path: path.to_string(), package: package.to_string(), parsed, decls, library }
    }
    pub fn tree(&self) -> N<'_> {
        N::new(&self.parsed.root, &self.parsed.toks)
    }
}

/// Associated functions, methods, fields, variants and constants declared directly in a class/enum/interface/mixin.
pub fn members_of(node: N) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for rule in ["classMember", "enumMember", "interfaceMember", "mixinMember", "implementMember"] {
        for m in node.child_nodes(rule) {
            if let Some(head) = m.find("methodHead") {
                if let Some(mn) = head.find("memberName") {
                    out.insert(mn.tokens()[0].text.clone());
                }
                continue;
            }
            if let Some(v) = m.find("variant") {
                out.insert(v.direct_tokens()[0].text.clone());
                continue;
            }
            if let Some(fd) = m.find("fieldDecl") {
                out.insert(fd.direct_tokens()[0].text.clone());
                continue;
            }
            if let Some(cd) = m.find("constDecl") {
                if let Some(t) = cd.direct_tokens().get(1) {
                    out.insert(t.text.clone());
                }
            }
        }
    }
    // Derivations generate associated entry points; the declaration checker judges their applicability and
    // identity (6.10, 6.13). Both conversion spellings retain `from` for v0.1 compatibility (X-552).
    let kids = node.kids();
    for (i, k) in kids.iter().enumerate() {
        if k.text_is("derive") {
            if let Some(list) = kids.get(i + 1).and_then(|c| c.node()) {
                if list.direct_tokens().iter().any(|t| t.text == "Default") {
                    out.insert("default".into());
                }
                if list.direct_tokens().iter().any(|t| t.text == "From" || t.text == "convert") {
                    out.insert("from".into());
                }
            }
        }
    }
    out
}

fn parse_file(path: &Path) -> Arc<Parsed> {
    let src = std::fs::read_to_string(path).unwrap_or_default();
    match parse_module(&src) {
        Ok(p) => Arc::new(p),
        Err(e) => panic!("reference library module {} does not parse: {:?}", path.display(), e),
    }
}

/// The reference library's parsed modules, read once per process: (module key, package, file, tree).
fn reference_modules() -> &'static Vec<(String, String, String, Arc<Parsed>)> {
    static R: OnceLock<Vec<(String, String, String, Arc<Parsed>)>> = OnceLock::new();
    R.get_or_init(|| {
        let lib = repo_root().join("reference").join("library");
        let mut out = Vec::new();
        for m in CORE_MODULES {
            let p = lib.join(format!("{m}.ft"));
            if p.exists() {
                out.push((format!("fortis.{m}"), "fortis".to_string(), p.display().to_string(), parse_file(&p)));
            }
        }
        for sub in ["kits", "examples"] {
            let d = lib.join(sub);
            if let Ok(rd) = std::fs::read_dir(&d) {
                let mut files: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|f| f.ends_with(".ft")).collect();
                files.sort();
                for f in files {
                    let key = f[..f.len() - 3].to_string();
                    let package = if sub == "kits" { "fortis".to_string() } else { format!("{}@0.0.0", key.rsplit_once('.').map_or(key.as_str(), |(a, _)| a)) };
                    out.push((key, package, d.join(&f).display().to_string(), parse_file(&d.join(&f))));
                }
            }
        }
        out
    })
}

pub struct Library {
    pub modules: BTreeMap<String, Arc<LibraryModule>>,
}

impl Library {
    pub fn new() -> Library {
        let mut modules = BTreeMap::new();
        for (key, package, file, parsed) in reference_modules() {
            let mut lm = LibraryModule::new(file, package, parsed.clone(), None);
            lm.path = file.clone();
            modules.insert(key.clone(), Arc::new(lm));
        }
        Library { modules }
    }
    pub fn prelude(&self) -> Option<&Arc<LibraryModule>> {
        self.modules.get("fortis.prelude")
    }
}

impl Default for Library {
    fn default() -> Self {
        Library::new()
    }
}

/// The libraries of one check: a single module uses one; a package set has one per package identity.
pub struct Libraries {
    pub libs: Vec<Library>,
}
