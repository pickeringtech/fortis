//! The compatibility set of the standard packages (Platform Specification 1.5; PLAT-01-01).
//!
//! Kits are versioned separately and tested only together within a published set
//! (`library/fortis/compatibility.yaml`). A project's dependency on a standard package (`fortis` or `fortis.…`) must
//! name a package of the set at the set's version; anything else would mix versions that were never tested together,
//! and is refused (`E-NAM-031`).

use std::collections::BTreeMap;
use std::path::Path;

use fortis_diag::{Json, Record};

pub const FILE: &str = "library/fortis/compatibility.yaml";

pub fn is_standard(name: &str) -> bool {
    name == "fortis" || name.starts_with("fortis.")
}

/// The set's name and its packages with their versions.
pub fn set() -> (String, BTreeMap<String, String>) {
    let text = std::fs::read_to_string(crate::library::repo_root().join(FILE)).unwrap_or_default();
    parse(&text)
}

pub fn parse(text: &str) -> (String, BTreeMap<String, String>) {
    // the quota of every input parser (PROG-SEC-01): an oversized set is read as empty, so every standard
    // dependency is refused rather than the file read without bound
    if text.len() > 1 << 20 {
        return (String::new(), BTreeMap::new());
    }
    let unquote = |s: &str| s.trim().trim_matches('"').to_string();
    let mut name = String::new();
    let mut packages = BTreeMap::new();
    let mut in_packages = false;
    for line in text.lines() {
        let body = line.split('#').next().unwrap_or("");
        if body.trim().is_empty() {
            continue;
        }
        if !body.starts_with(' ') {
            in_packages = body.trim() == "packages:";
            if let Some(v) = body.strip_prefix("set:") {
                name = unquote(v);
            }
        } else if in_packages {
            if let Some((k, v)) = body.trim().split_once(':') {
                packages.insert(k.trim().to_string(), unquote(v));
            }
        }
    }
    (name, packages)
}

/// `E-NAM-031` when `dep@version` is not in the compatibility set.
pub fn check(dep: &str, version: &str, manifest: &Path) -> Option<Record> {
    let (name, packages) = set();
    let file = crate::resolve::rel(manifest);
    let fields = |cands: Vec<String>| Json::obj([("name", Json::str(dep)), ("candidates", Json::strs(cands))]);
    match packages.get(dep) {
        Some(v) if v == version => None,
        Some(v) => Some(Record::new(
            "E-NAM-031",
            format!("{dep} {version} is not in the compatibility set {name}, which has {dep} {v}: standard packages are tested only together within a set"),
            "PLAT-01-01",
            &file,
            1,
            1,
            fields(vec![format!("{dep}: \"{v}\"")]),
        )),
        None => Some(Record::new(
            "E-NAM-031",
            format!("{dep} is not a standard package of the compatibility set {name}"),
            "PLAT-01-01",
            &file,
            1,
            1,
            fields(packages.keys().cloned().collect()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_set_reads_and_pins_versions() {
        let (name, p) = set();
        assert_eq!(name, "1.0");
        assert_eq!(p.get("fortis").map(|s| s.as_str()), Some("1.0"));
        assert!(check("fortis.http", "1.0", Path::new("p.yaml")).is_none());
        assert_eq!(check("fortis.http", "2.0", Path::new("p.yaml")).map(|r| r.code), Some("E-NAM-031".to_string()));
        assert_eq!(check("fortis.nothing", "1.0", Path::new("p.yaml")).map(|r| r.code), Some("E-NAM-031".to_string()));
    }
}
