//! Target feature sets (Platform Specification 1.5; PROG-ARC-03, PROG-ARC-06, PLAT-01-03).
//!
//! A target feature set (`core`, `alloc`, `host`, `async`, `web`, `browser`, `desktop`) decides which standard
//! modules exist for a compilation target. It is neither an effect nor a capability: those are facts about a
//! function and run-time values, and they are represented separately in the IR, the build manifest and the
//! diagnostics. A module that does not exist on the selected target is an error at `check` (`E-NAM-030`), before
//! any native compilation starts.

use std::collections::BTreeSet;

use fortis_diag::{Json, Record};

/// The feature set each standard module needs (Platform 1.5): the longest matching module prefix decides.
const MODULES: &[(&str, &str)] = &[
    ("fortis.prelude", "core"),
    ("fortis.text", "core"),
    ("fortis.collections", "core"),
    ("fortis.numeric", "core"),
    ("fortis.time", "core"),
    ("fortis.money", "core"),
    ("fortis.codec", "alloc"),
    ("fortis.host", "host"),
    ("fortis.runtime", "host"),
    ("fortis.runtime.executor", "async"),
    ("fortis.di", "host"),
    ("fortis.data", "host"),
    ("fortis.telemetry", "host"),
    ("fortis.security", "host"),
    ("fortis.config", "host"),
    ("fortis.packages", "host"),
    ("fortis.http", "web"),
    ("fortis.web", "web"),
    ("fortis.web.spa", "browser"),
    ("fortis.web.pwa", "browser"),
    ("fortis.desktop", "desktop"),
];

/// The Language 1.0 targets and their feature sets (Programme, hosts and targets).
pub const TARGETS: &[(&str, &[&str])] = &[
    ("x86_64-unknown-linux-gnu", &["core", "alloc", "host", "async", "web"]),
    ("x86_64-unknown-linux-musl", &["core", "alloc", "host", "async", "web"]),
    ("aarch64-unknown-linux-gnu", &["core", "alloc", "host", "async", "web"]),
    ("aarch64-unknown-linux-musl", &["core", "alloc", "host", "async", "web"]),
    ("aarch64-apple-darwin", &["core", "alloc", "host", "async", "web"]),
    ("x86_64-pc-windows-msvc", &["core", "alloc", "host", "async", "web"]),
    ("wasm32-wasip1", &["core", "alloc"]),
    ("thumbv7em-none-eabihf", &["core"]),
    ("wasm32-unknown-unknown", &["core", "alloc", "browser"]),
];

/// The target this toolchain runs on.
pub fn host_target() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        _ => "x86_64-unknown-linux-gnu",
    }
}

/// A target's feature set, or None for a target this toolchain does not know.
pub fn features(target: &str) -> Option<BTreeSet<String>> {
    TARGETS.iter().find(|(t, _)| *t == target).map(|(_, fs)| fs.iter().map(|f| f.to_string()).collect())
}

/// The feature set a module needs, or None for a module that is not a standard module.
pub fn module_feature(module: &str) -> Option<&'static str> {
    MODULES.iter().filter(|(m, _)| module == *m || module.starts_with(&format!("{m}."))).max_by_key(|(m, _)| m.len()).map(|(_, f)| *f)
}

/// `E-NAM-030` for each import of a standard module that does not exist on `target`.
pub fn check_imports(parsed: &fortis_syntax::Parsed, file: &str, target: &str) -> Vec<Record> {
    let Some(have) = features(target) else {
        return vec![Record::new("E-NAM-030", format!("unknown target `{target}`; known targets: {}", TARGETS.iter().map(|t| t.0).collect::<Vec<_>>().join(", ")), "PLAT-01-03", file, 1, 1, fields(target, "", vec![]))];
    };
    let root = fortis_syntax::view::N::new(&parsed.root, &parsed.toks);
    let mut out = Vec::new();
    for imp in root.find_all("import") {
        let path: Vec<String> = imp.child_node("dottedPath").map(|m| m.tokens().iter().filter(|t| t.text != ".").map(|t| t.text.clone()).collect()).unwrap_or_default();
        let module = path.join(".");
        if let Some(need) = module_feature(&module) {
            if !have.contains(need) {
                let tok = imp.first_token();
                out.push(Record::new(
                    "E-NAM-030",
                    format!("`{module}` does not exist on target {target}: it needs the `{need}` feature set, and the target has {}", have.iter().cloned().collect::<Vec<_>>().join(", ")),
                    "PLAT-01-03",
                    file,
                    tok.map_or(1, |t| t.line),
                    tok.map_or(1, |t| t.col),
                    fields(target, &module, have.iter().cloned().collect()),
                ));
            }
        }
    }
    out
}

fn fields(target: &str, module: &str, have: Vec<String>) -> Json {
    Json::obj([("name", Json::str(module)), ("candidates", Json::strs(have)), ("target", Json::str(target)), ("feature_set", Json::str(module_feature(module).unwrap_or("")))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_prefix_decides() {
        assert_eq!(module_feature("fortis.runtime.executor"), Some("async"));
        assert_eq!(module_feature("fortis.runtime"), Some("host"));
        assert_eq!(module_feature("fortis.web.spa"), Some("browser"));
        assert_eq!(module_feature("fortis.web.ssr"), Some("web"));
        assert_eq!(module_feature("acme.billing"), None);
    }

    #[test]
    fn an_absent_module_is_an_error_naming_the_feature_set() {
        let p = fortis_syntax::parse_module("import fortis.host { Host }\nimport fortis.text\n").unwrap();
        let r = check_imports(&p, "m.ft", "thumbv7em-none-eabihf");
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].code.as_str(), r[0].line), ("E-NAM-030", 1));
        assert!(check_imports(&p, "m.ft", "x86_64-unknown-linux-gnu").is_empty());
    }
}
