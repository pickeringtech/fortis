//! project.yaml and package loading (Language Reference 4.1-4.3, 4.6): a port of conformance/tools/project.py.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fortis_syntax::view::{declarations, imports, DeclInfo, ImportInfo, N};
use fortis_syntax::{parse_module_located, Parsed};

pub const MANIFEST_KEYS: &[&str] = &["schema", "name", "version", "exports", "dependencies", "entry", "root", "kits", "targets"];

#[derive(Debug)]
pub struct ManifestError(pub String);

#[derive(Clone, Debug)]
pub enum MValue {
    Str(String),
    List(Vec<String>),
    Map(Vec<(String, String)>),
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    let b: Vec<char> = v.chars().collect();
    if b.len() >= 2 && b[0] == b[b.len() - 1] && (b[0] == '"' || b[0] == '\'') {
        return b[1..b.len() - 1].iter().collect();
    }
    v.to_string()
}

/// The manifest quotas (PROG-SEC-01): a manifest past them is refused with `E-LIM-003` before it is read.
pub const MAX_MANIFEST_BYTES: usize = 1 << 20;
pub const MAX_MANIFEST_LINES: usize = 10_000;

pub fn read_manifest(path: &Path) -> Result<Vec<(String, MValue)>, ManifestError> {
    let text = std::fs::read_to_string(path).map_err(|e| ManifestError(format!("{}: {e}", path.display())))?;
    if text.len() > MAX_MANIFEST_BYTES || text.lines().count() > MAX_MANIFEST_LINES {
        return Err(ManifestError(format!(
            "E-LIM-003: {}: a manifest of {} bytes and {} lines exceeds the budget of {MAX_MANIFEST_BYTES} bytes and {MAX_MANIFEST_LINES} lines",
            path.display(),
            text.len(),
            text.lines().count()
        )));
    }
    let mut out: Vec<(String, MValue)> = Vec::new();
    let mut current: Option<usize> = None;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        if line.starts_with("  ") && current.is_some() {
            let (k, v) = line.trim().split_once(':').unwrap_or((line.trim(), ""));
            if let MValue::Map(m) = &mut out[current.unwrap()].1 {
                m.push((k.trim().to_string(), unquote(v)));
            }
            continue;
        }
        current = None;
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        let (k, v) = (k.trim().to_string(), v.trim());
        if v.is_empty() {
            out.push((k, MValue::Map(vec![])));
            current = Some(out.len() - 1);
        } else if v.starts_with('[') {
            let inner = v.trim_matches(|c| c == '[' || c == ']');
            out.push((k, MValue::List(inner.split(',').filter(|x| !x.trim().is_empty()).map(unquote).collect())));
        } else {
            out.push((k, MValue::Str(unquote(v))));
        }
    }
    for req in ["name", "version"] {
        if !out.iter().any(|(k, _)| k == req) {
            return Err(ManifestError(format!("{}: manifest lacks {req}", path.display())));
        }
    }
    for (k, _) in &out {
        if !MANIFEST_KEYS.contains(&k.as_str()) {
            return Err(ManifestError(format!(
                "{}: `{k}` is not a key of fortis.project/v1; configuration cannot add or remove checks, contracts or predicates (PROG-FND-04)",
                path.display()
            )));
        }
    }
    Ok(out)
}

pub struct Module {
    pub path: String,
    pub file: PathBuf,
    pub parsed: Option<Arc<Parsed>>,
    pub error: Option<(String, usize, usize)>,
}

impl Module {
    pub fn tree(&self) -> Option<N<'_>> {
        self.parsed.as_ref().map(|p| N::new(&p.root, &p.toks))
    }
    pub fn decls(&self) -> Vec<DeclInfo<'_>> {
        self.tree().map(declarations).unwrap_or_default()
    }
    pub fn imports(&self) -> Vec<ImportInfo<'_>> {
        self.tree().map(imports).unwrap_or_default()
    }
}

pub struct Package {
    pub dir: PathBuf,
    pub name: String,
    pub version: String,
    pub exports: Vec<String>,
    pub entry: Option<String>,
    pub root: Option<String>,
    pub dependencies: Vec<(String, String)>,
    pub modules: BTreeMap<String, Module>,
}

impl Package {
    pub fn load(dir: &Path) -> Result<Package, ManifestError> {
        let m = read_manifest(&dir.join("project.yaml"))?;
        let get = |k: &str| m.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        let s = |v: Option<MValue>| match v {
            Some(MValue::Str(s)) => Some(s),
            _ => None,
        };
        let exports = match get("exports") {
            Some(MValue::List(l)) => l,
            _ => vec![],
        };
        let dependencies = match get("dependencies") {
            Some(MValue::Map(m)) => m,
            _ => vec![],
        };
        let mut modules = BTreeMap::new();
        let src = dir.join("src");
        let mut stack = vec![src.clone()];
        while let Some(d) = stack.pop() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&d).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map_or(false, |e| e == "ft") {
                    let rel = p.strip_prefix(&src).unwrap().with_extension("");
                    let key = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join(".");
                    modules.insert(key.clone(), Module { path: key, file: p.clone(), parsed: None, error: None });
                }
            }
        }
        Ok(Package {
            dir: dir.to_path_buf(),
            name: s(get("name")).unwrap_or_default(),
            version: s(get("version")).unwrap_or_default(),
            exports,
            entry: s(get("entry")),
            root: s(get("root")),
            dependencies,
            modules,
        })
    }

    pub fn identity(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    pub fn parse_all(&mut self) {
        for m in self.modules.values_mut() {
            // a module is read as the core sees it: elaborated when it uses the DI kit's heads (fortis-di)
            let src = fortis_di::source(std::path::Path::new(&m.file)).unwrap_or_default();
            match parse_module_located(&src) {
                Ok(p) => m.parsed = Some(Arc::new(p)),
                Err(e) => m.error = Some(e),
            }
        }
    }
}

pub fn find_package_dir(search: &Path, name: &str, version: Option<&str>) -> Option<PathBuf> {
    if let Some(v) = version {
        let c = search.join(format!("{name}@{v}"));
        if c.is_dir() {
            return Some(c);
        }
    }
    let c = search.join(name);
    if c.is_dir() {
        Some(c)
    } else {
        None
    }
}
