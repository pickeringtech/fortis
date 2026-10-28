//! Effect summaries (DEL-19): a port of checker/effects.py.

use std::collections::{BTreeMap, BTreeSet};

use fortis_syntax::Token;

pub const EFFECTS: &[&str] = &["alloc", "io", "mutate", "nondet", "suspend", "block", "fail", "recovers"];
pub const PURE_FORBIDS: &[&str] = &["io", "mutate", "nondet", "suspend", "block"];
pub const AUTHORITIES: &[&str] = &["FileSystem", "Network", "Clock", "Random", "Environment", "Process", "Supervisor", "Console", "SecretAccess", "RawSql"];

/// The effects a standard package's data may give an operation, in the order a summary adds them; `fail`, `suspend` and
/// `recovers` change a function's type, so only its modifiers give them (11.2).
pub const DECLARABLE: &[&str] = &["nondet", "block", "mutate", "alloc", "io"];

/// An operation as the effect data names it: (module, owner or None for a free function, name).
pub type OpKey = (String, Option<String>, String);

/// Every standard package's effect data (reference/library/effects/<package>.json; D-112), read once per process: the
/// one source, shared with checker/effects.py, for the effects a trusted operation's signature cannot show. Malformed
/// data stops the checker.
pub fn effect_data() -> &'static BTreeMap<OpKey, Vec<String>> {
    static D: std::sync::OnceLock<BTreeMap<OpKey, Vec<String>>> = std::sync::OnceLock::new();
    D.get_or_init(|| {
        let dir = fortis_names::library::repo_root().join("reference").join("library").join("effects");
        let packages: Vec<String> = fortis_names::compat::set().1.into_keys().collect();
        load_effect_data(&dir, &packages).unwrap_or_else(|e| panic!("{e}"))
    })
}

/// The standard package a module belongs to: the longest package name that is the module or prefixes it.
pub fn owning_package<'p>(module: &str, packages: &'p [String]) -> Option<&'p String> {
    packages.iter().filter(|p| module == p.as_str() || module.starts_with(&format!("{p}."))).max_by_key(|p| p.len())
}

/// Read the effect data of `dir`: one file per standard package, `<package>.json`, holding rows for that package's
/// modules only.
pub fn load_effect_data(dir: &std::path::Path, packages: &[String]) -> Result<BTreeMap<OpKey, Vec<String>>, String> {
    use fortis_diag::Json;
    let mut rows = BTreeMap::new();
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("effect data {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|f| f.ends_with(".json"))
        .collect();
    files.sort();
    for f in files {
        let where_ = format!("effect data {f}");
        let text = std::fs::read_to_string(dir.join(&f)).map_err(|e| format!("{where_}: {e}"))?;
        let d = Json::parse(&text).map_err(|e| format!("{where_}: {e}"))?;
        let keys_ok = |j: &Json, allowed: &[&str]| matches!(j, Json::Obj(m) if m.keys().all(|k| allowed.contains(&k.as_str())));
        if d.get("schema").and_then(|s| s.as_str()) != Some("fortis.effects/v1") || !keys_ok(&d, &["schema", "package", "about", "operations"]) {
            return Err(format!("{where_}: not a fortis.effects/v1 document"));
        }
        let pkg = d.get("package").and_then(|p| p.as_str()).unwrap_or("").to_string();
        if !packages.contains(&pkg) || f != format!("{pkg}.json") {
            return Err(format!("{where_}: names package {pkg:?}, which is not a standard package of the compatibility set or not the file's name"));
        }
        let ops = d.get("operations").and_then(|o| o.as_arr()).ok_or_else(|| format!("{where_}: no operations list"))?;
        for r in ops {
            let has = |k: &str| r.get(k).is_some();
            if !keys_ok(r, &["module", "owner", "name", "effects", "why"]) || !(has("module") && has("owner") && has("name") && has("effects")) {
                return Err(format!("{where_}: a row needs module, owner, name and effects (and may give why): {}", r.to_line()));
            }
            let module = r.get("module").and_then(|m| m.as_str()).unwrap_or("");
            if owning_package(module, packages) != Some(&pkg) {
                return Err(format!("{where_}: module {module:?} is not one of package {pkg}'s"));
            }
            let owner = match r.get("owner") {
                Some(Json::Null) => None,
                Some(Json::Str(o)) if !o.is_empty() => Some(o.clone()),
                _ => return Err(format!("{where_}: malformed owner, name or why in {}", r.to_line())),
            };
            let name = match r.get("name") {
                Some(Json::Str(n)) if !n.is_empty() => n.clone(),
                _ => return Err(format!("{where_}: malformed owner, name or why in {}", r.to_line())),
            };
            if r.get("why").map_or(false, |w| w.as_str().is_none()) {
                return Err(format!("{where_}: malformed owner, name or why in {}", r.to_line()));
            }
            let effs: Vec<String> = r.get("effects").and_then(|e| e.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            let n_given = r.get("effects").and_then(|e| e.as_arr()).map_or(0, |a| a.len());
            let distinct: std::collections::BTreeSet<&String> = effs.iter().collect();
            if effs.is_empty() || effs.len() != n_given || distinct.len() != effs.len() || effs.iter().any(|e| !DECLARABLE.contains(&e.as_str())) {
                return Err(format!("{where_}: {module} {}.{name}: effects must be a non-empty list of distinct {}", owner.clone().unwrap_or("None".into()), DECLARABLE.join(", ")));
            }
            let key = (module.to_string(), owner.clone(), name.clone());
            if rows.contains_key(&key) {
                return Err(format!("{where_}: {module} {}.{name} is listed twice", owner.unwrap_or("None".into())));
            }
            rows.insert(key, DECLARABLE.iter().filter(|e| effs.iter().any(|x| x == *e)).map(|e| e.to_string()).collect());
        }
    }
    Ok(rows)
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    /// effect -> "proved" | "trusted", in the order effects were first added
    pub effects: Vec<(String, String)>,
    pub footprint: BTreeSet<String>,
    pub sites: BTreeMap<String, Token>,
    pub authorities: BTreeSet<String>,
    pub needs_auth: bool,
    /// effects that are not certain (PROG-DIA-02, X-132): `possible` when only a callback's type permits the
    /// effect, `unknown` when it comes through a foreign `trusted` function that does not claim its absence; an
    /// effect absent here is certain (proved, or declared by a trusted summary)
    pub uncertain: BTreeMap<String, String>,
}

impl Summary {
    /// Add an effect that is not certain; a certain occurrence of the same effect, before or after, wins.
    pub fn add_uncertain(&mut self, effect: &str, label: &str, kind: &str, tok: Option<&Token>) {
        let certain = self.has(effect) && !self.uncertain.contains_key(effect);
        self.add_keeping(effect, label, tok);
        if !certain {
            let k = self.uncertain.entry(effect.to_string()).or_insert_with(|| kind.to_string());
            // a possible effect is stronger evidence than an unknown one
            if kind == "possible" {
                *k = kind.to_string();
            }
        }
    }
    /// The status a claim excluding `effect` has (Programme 7.1): violated, unproved or unknown.
    pub fn status(&self, effect: &str) -> &'static str {
        match self.uncertain.get(effect).map(|s| s.as_str()) {
            Some("possible") => "unproved",
            Some(_) => "unknown",
            None => "violated",
        }
    }
    pub fn has(&self, e: &str) -> bool {
        self.effects.iter().any(|(k, _)| k == e)
    }
    pub fn add(&mut self, effect: &str, label: &str, tok: Option<&Token>) {
        self.uncertain.remove(effect);
        self.add_keeping(effect, label, tok);
    }
    fn add_keeping(&mut self, effect: &str, label: &str, tok: Option<&Token>) {
        match self.effects.iter_mut().find(|(k, _)| k == effect) {
            None => self.effects.push((effect.to_string(), label.to_string())),
            Some(e) => {
                if e.1 == "trusted" && label == "proved" {
                    e.1 = label.to_string();
                }
            }
        }
        if let Some(t) = tok {
            self.sites.entry(effect.to_string()).or_insert_with(|| t.clone());
        }
    }
    pub fn imports(&mut self, other: &Summary, tok: Option<&Token>, skip: &[&str]) {
        for (e, lab) in &other.effects {
            if !skip.contains(&e.as_str()) {
                match other.uncertain.get(e) {
                    Some(kind) => self.add_uncertain(e, lab, kind, tok),
                    None => self.add(e, lab, tok),
                }
            }
        }
        self.authorities.extend(other.authorities.iter().cloned());
        self.needs_auth = self.needs_auth || other.needs_auth;
    }
    pub fn remove(&mut self, e: &str) {
        self.effects.retain(|(k, _)| k != e);
        self.sites.remove(e);
        self.uncertain.remove(e);
    }
    pub fn effect_set(&self) -> BTreeSet<String> {
        self.effects.iter().map(|(k, _)| k.clone()).collect()
    }
}

impl Summary {
    /// The reference's `Summary.show()`: effects sorted, `(trusted)` marked, then the write footprint.
    pub fn show(&self) -> String {
        let mut es: Vec<&(String, String)> = self.effects.iter().collect();
        es.sort();
        let inner: Vec<String> = es.iter().map(|(e, l)| if l == "proved" { e.clone() } else { format!("{e} (trusted)") }).collect();
        let mut s = format!("{{{}}}", inner.join(", "));
        if !self.footprint.is_empty() {
            let fp: Vec<String> = self.footprint.iter().map(|f| format!("'{f}'")).collect();
            s.push_str(&format!(" footprint [{}]", fp.join(", ")));
        }
        s
    }
}

/// Every function's summary in one stable text form (compared byte for byte with checker/effects.py).
pub fn summaries_text(summaries: &std::collections::BTreeMap<String, Summary>) -> String {
    let mut out = String::new();
    for (label, s) in summaries {
        let auth: Vec<String> = s.authorities.iter().map(|a| format!("'{a}'")).collect();
        out.push_str(&format!("{label}: {} authorities [{}] needs_auth {}\n", s.show(), auth.join(", "), if s.needs_auth { "True" } else { "False" }));
    }
    out
}

/// The summary of a trusted (bodiless) library signature: `name` of `owner` (None for a free function) declared in
/// `module`, with the Authority types its parameters take.
pub fn library_summary(module: Option<&str>, owner: Option<&str>, name: &str, modifiers: &[String], implements_authority: bool, receiver: Option<&str>, authority_params: &BTreeSet<String>) -> Summary {
    let mut s = Summary::default();
    let has = |m: &str| modifiers.iter().any(|x| x == m);
    if has("try") {
        s.add("fail", "trusted", None);
    }
    if has("async") {
        s.add("suspend", "trusted", None);
    }
    if has("recovers") {
        s.add("recovers", "trusted", None);
    }
    if let Some(module) = module {
        if let Some(effs) = effect_data().get(&(module.to_string(), owner.map(|o| o.to_string()), name.to_string())) {
            for e in effs {
                s.add(e, "trusted", None);
                if e == "io" {
                    s.needs_auth = true;
                }
            }
        }
    }
    // a method of a capability or authority, or an operation that takes one, performs I/O through it (11.6; D-112)
    if !has("pure") && ((implements_authority && receiver.is_some()) || !authority_params.is_empty()) {
        s.add("io", "trusted", None);
        s.needs_auth = true;
        s.authorities.extend(authority_params.iter().cloned());
    }
    if receiver == Some("editable self") && !has("pure") {
        s.footprint.insert("self".into());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, f: &str, text: &str) {
        std::fs::write(dir.join(f), text).unwrap();
    }

    /// The shipped data is well formed and moves the old tables' rows: spot checks of each effect.
    #[test]
    fn the_shipped_data_loads() {
        let d = effect_data();
        let get = |m: &str, o: Option<&str>, n: &str| d.get(&(m.to_string(), o.map(|x| x.to_string()), n.to_string())).cloned().unwrap_or_default();
        assert_eq!(get("fortis.host", Some("Clock"), "now"), vec!["nondet"]);
        assert_eq!(get("fortis.prelude", Some("Mutex"), "lockBlocking"), vec!["block"]);
        assert_eq!(get("fortis.host", Some("FileSystem"), "readText"), vec!["block", "alloc"]);
        assert_eq!(get("fortis.prelude", Some("AtomicInt"), "store"), vec!["mutate"]);
        assert_eq!(get("fortis.prelude", None, "annotate"), vec!["alloc"]);
        assert!(get("fortis.prelude", Some("List"), "count").is_empty());
    }

    /// Every row names an operation its module declares in the reference library (library_check.py also checks that a
    /// core module's is a `trusted` intrinsic, without a Fortis body).
    #[test]
    fn every_row_names_a_library_operation() {
        let lib = fortis_names::Library::new();
        for (module, owner, name) in effect_data().keys() {
            let m = lib.modules.get(module).unwrap_or_else(|| panic!("effect data names module {module}, which the reference library lacks"));
            let found = match owner {
                Some(o) => m.decls.get(o).map_or(false, |d| d.members.contains(name)),
                None => m.decls.get(name).map_or(false, |d| d.kind == "func"),
            };
            assert!(found, "effect data names {module} {}.{name}, which the module does not declare", owner.clone().unwrap_or_default());
        }
    }

    /// Malformed data is refused, naming the file.
    #[test]
    fn malformed_data_is_refused() {
        let dir = std::env::temp_dir().join(format!("fortis-effects-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pk = vec!["fortis".to_string(), "fortis.runtime".to_string()];
        let row = |r: &str| format!("{{\"schema\": \"fortis.effects/v1\", \"package\": \"fortis\", \"operations\": [{r}]}}");
        let cases = [
            ("fortis.json", row(r#"{"module": "fortis.host", "owner": "Clock", "name": "now", "effects": ["nondet"]}"#), true),
            ("fortis.json", row(r#"{"module": "fortis.host", "owner": "Clock", "name": "now", "effects": ["fail"]}"#), false),
            ("fortis.json", row(r#"{"module": "fortis.host", "owner": "Clock", "name": "now", "effects": []}"#), false),
            ("fortis.json", row(r#"{"module": "fortis.runtime", "owner": "App", "name": "run", "effects": ["block"]}"#), false),
            ("fortis.json", row(r#"{"module": "acme.app", "owner": null, "name": "go", "effects": ["io"]}"#), false),
            ("fortis.json", row(r#"{"module": "fortis.host", "owner": "Clock", "name": "now", "effects": ["nondet"], "extra": 1}"#), false),
            ("fortis.json", row(r#"{"module": "fortis.host", "owner": "Clock", "name": "now", "effects": ["nondet"]}, {"module": "fortis.host", "owner": "Clock", "name": "now", "effects": ["alloc"]}"#), false),
            ("acme.json", row(r#"{"module": "fortis.host", "owner": "Clock", "name": "now", "effects": ["nondet"]}"#), false),
        ];
        for (i, (f, text, ok)) in cases.iter().enumerate() {
            for e in std::fs::read_dir(&dir).unwrap() {
                std::fs::remove_file(e.unwrap().path()).unwrap();
            }
            write(&dir, f, text);
            assert_eq!(load_effect_data(&dir, &pk).is_ok(), *ok, "case {i}: {text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
