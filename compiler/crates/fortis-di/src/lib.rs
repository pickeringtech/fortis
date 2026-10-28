//! fortis-di: the Dependency injection kit's validator and elaboration (Platform 3; spikes/SP-08).
//!
//! `inject`, `provide` and `context` become ordinary declarations before the core checker runs: an `inject` slot
//! becomes a field (so the class's generated constructor takes it, and tests construct the consumer by hand), and a
//! `context` becomes a class per scope level with factory methods. The kit's own rules are about the provider
//! graph only (selection, placement, cycles); ownership, effects and failures of the wiring are the core's, judged
//! on the elaborated source, and a core diagnostic inside generated code is mapped back to the declaration it was
//! generated from.
//!
//! `source(path)` is what every reader of a module calls: a module that uses neither head reads as written.

pub mod emit;
pub mod plan;
pub mod scan;
pub mod schema;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use fortis_diag::{Json, Record};

/// A finding of the kit's validator, located in the module being elaborated.
#[derive(Clone, Debug)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
    pub rule: &'static str,
    pub line: usize,
    pub col: usize,
    pub consumer: String,
    pub slot: String,
    pub candidates: Vec<String>,
    /// an edit that repairs the finding: (line, column, inserted text) and the correction's kind
    pub correction: Option<(usize, usize, String, &'static str)>,
}

impl Problem {
    pub fn new(code: &'static str, message: impl Into<String>, rule: &'static str, line: usize, col: usize) -> Problem {
        Problem { code, message: message.into(), rule, line, col, consumer: String::new(), slot: String::new(), candidates: vec![], correction: None }
    }
    pub fn with(mut self, consumer: &str, slot: &str, candidates: Vec<String>) -> Problem {
        self.consumer = consumer.into();
        self.slot = slot.into();
        self.candidates = candidates;
        self
    }
    pub fn fix(mut self, line: usize, col: usize, text: &str, kind: &'static str) -> Problem {
        self.correction = Some((line, col, text.into(), kind));
        self
    }
    pub fn record(&self, file: &str) -> Record {
        let mut r = Record::new(
            self.code,
            self.message.clone(),
            self.rule,
            file,
            self.line,
            self.col,
            Json::obj([("consumer", Json::str(&self.consumer)), ("slot", Json::str(&self.slot)), ("candidates", Json::strs(self.candidates.clone()))]),
        );
        if let Some((line, col, text, kind)) = &self.correction {
            let edit = Json::obj([
                ("file", Json::str(file)),
                ("line", Json::Int(*line as i64)),
                ("column", Json::Int(*col as i64)),
                ("end_line", Json::Int(*line as i64)),
                ("end_column", Json::Int(*col as i64)),
                ("replacement", Json::str(text)),
            ]);
            r.correction = Some(Json::obj([("edits", Json::Arr(vec![edit])), ("kind", Json::str(*kind))]));
        }
        r
    }
}

pub struct Elaborated {
    pub text: String,
    /// for each line of `text` (from 1), the line of the source it comes from
    pub origins: Vec<usize>,
    /// the source's line count: a line beyond it is generated
    pub source_lines: usize,
    pub plans: Vec<Json>,
    pub problems: Vec<Problem>,
}

/// Elaborate one module, given every module of its package (the module itself included) and the package's
/// `name@version`.
pub fn elaborate(file: &str, text: &str, package: &[scan::Module], package_id: &str) -> Elaborated {
    let here = scan::module(file, text);
    let mut problems = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<(String, usize)> = lines.iter().enumerate().map(|(i, l)| (l.to_string(), i + 1)).collect();
    // `inject` slots become fields where they are written: only the word `inject` (and `lazy`) leaves the line, so
    // no other line moves and a slot keeps its place among the fields (Platform 3.1, as revised by SP-08)
    for c in &here.classes {
        for s in &c.slots {
            if s.lazy {
                problems.push(Problem::new("E-DI-011", format!("`inject lazy {}`: lazy edges are not in DI schema v1 (Platform 3.4)", s.name), "PLAT-03-03", s.line, s.col).with(&c.name, &s.name, vec![]));
            }
            // `inject` and a following `lazy` leave the line at the slot's own column, wherever the slot is on it
            let chars: Vec<char> = out[s.line - 1].0.chars().collect();
            let at = s.col - 1;
            let word = |from: usize, w: &str| chars.get(from..from + w.len()).map_or(false, |x| x.iter().collect::<String>() == w);
            if word(at, "inject") {
                let mut end = at + "inject".len();
                while end < chars.len() && chars[end] == ' ' {
                    end += 1;
                }
                if word(end, "lazy") && chars.get(end + 4).map_or(false, |c| *c == ' ') {
                    end += 4;
                    while end < chars.len() && chars[end] == ' ' {
                        end += 1;
                    }
                }
                out[s.line - 1].0 = chars[..at].iter().chain(chars[end..].iter()).collect();
            }
        }
    }
    // contexts: parsed, resolved, removed from the text and generated after it
    let mut contexts = BTreeMap::new();
    let declared: std::collections::BTreeSet<&str> = here.classes.iter().map(|c| c.name.as_str()).collect();
    // the classes every context generates, so two contexts cannot generate one name (`C` and `CApplication`)
    let mut generated_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for h in &here.contexts {
        for g in std::iter::once(h.name.clone()).chain(std::iter::once(format!("{}Provenance", h.name))).chain(schema::LEVELS.iter().filter(|l| **l != "transient").map(|l| plan::level_class(&h.name, l))) {
            generated_by.entry(g).or_default().push(h.name.clone());
        }
    }
    for h in &here.contexts {
        let c = schema::parse(h, &mut problems);
        for l in h.line..=h.end_line.min(out.len()) {
            out[l - 1].0 = String::new();
        }
        if contexts.contains_key(&c.name) {
            problems.push(Problem::new("E-DI-010", format!("the context {} is declared twice", c.name), "PLAT-03-01", h.line, 1));
            continue;
        }
        // the names the elaboration declares may not be the module's own
        let mut generated = vec![c.name.clone(), format!("{}Provenance", c.name)];
        generated.extend(schema::LEVELS.iter().filter(|l| **l != "transient").map(|l| plan::level_class(&c.name, l)));
        if let Some(clash) = generated.iter().find(|g| declared.contains(g.as_str())) {
            problems.push(Problem::new("E-DI-010", format!("the context {} declares the class {clash}, which the module declares already", c.name), "PLAT-03-01", h.line, 1));
            continue;
        }
        if let Some(clash) = generated.iter().find(|g| generated_by.get(*g).map_or(false, |v| v.iter().any(|o| *o != c.name))) {
            let other = generated_by[clash].iter().find(|o| **o != c.name).cloned().unwrap_or_default();
            problems.push(Problem::new("E-DI-010", format!("the contexts {} and {other} both declare the class {clash}", c.name), "PLAT-03-01", h.line, 1));
            continue;
        }
        contexts.insert(c.name.clone(), c);
    }
    let w = plan::World::new(&here, package, package_id.split('@').next().unwrap_or(""));
    let mut plans = Vec::new();
    let mut generated: Vec<(String, usize)> = Vec::new();
    for c in contexts.values() {
        if let Some(r) = plan::resolve(c, &contexts, &w, &mut problems) {
            plans.push(plan::plan_json(&r, package_id));
            let provenance = here.imports.iter().any(|i| i.path == ["fortis", "di", "provenance"]);
            let mut g = emit::Gen::new(&r, &w, provenance);
            generated.push((String::new(), c.line));
            generated.extend(g.lines());
        }
    }
    let source_lines = out.len();
    if !generated.is_empty() {
        out.push((format!("// fortis: generated by fortis.di (DI schema v1) from the contexts of this module"), source_lines));
        out.extend(generated);
    }
    Elaborated { text: out.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join("\n"), origins: out.iter().map(|x| x.1).collect(), source_lines, plans, problems }
}

/// The package a module belongs to: (root, `name@version`), or None for a lone file.
fn package_of(path: &Path) -> Option<(PathBuf, String)> {
    let mut d = path.parent();
    while let Some(dir) = d {
        let manifest = dir.join("project.yaml");
        if manifest.exists() {
            let text = std::fs::read_to_string(&manifest).unwrap_or_default();
            let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}:")).map(|v| v.trim().trim_matches('"').to_string())).unwrap_or_default();
            return Some((dir.to_path_buf(), format!("{}@{}", field("name"), field("version"))));
        }
        d = dir.parent();
    }
    None
}

fn ft_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut es: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        es.sort();
        for p in es {
            if p.is_dir() {
                ft_files(&p, out);
            } else if p.extension().map_or(false, |e| e == "ft") {
                out.push(p);
            }
        }
    }
}

struct Entry {
    origins: Vec<usize>,
    source_lines: usize,
    records: Vec<Record>,
    plans: Vec<Json>,
}

fn registry() -> &'static Mutex<HashMap<String, Entry>> {
    static R: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(path: &Path) -> String {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string()
}

/// A module's text as the core reads it: elaborated when it uses the kit's syntax, as written otherwise.
pub fn source(path: &Path) -> std::io::Result<String> {
    let text = std::fs::read_to_string(path)?;
    Ok(source_text(path, text))
}

/// As `source`, for text already read.
pub fn source_text(path: &Path, text: String) -> String {
    if !scan::uses_di(&text) {
        return text;
    }
    let file = path.display().to_string();
    let (modules, id) = match package_of(path) {
        Some((root, id)) => {
            let mut files = Vec::new();
            ft_files(&root.join("src"), &mut files);
            let ms: Vec<scan::Module> = files.iter().filter_map(|f| std::fs::read_to_string(f).ok().map(|t| scan::module(&f.display().to_string(), &t))).collect();
            (ms, id)
        }
        None => (vec![], String::new()),
    };
    let e = elaborate(&file, &text, &modules, &id);
    let records = e.problems.iter().map(|p| p.record(&file)).collect();
    registry().lock().unwrap().insert(key(path), Entry { origins: e.origins, source_lines: e.source_lines, records, plans: e.plans });
    e.text
}

/// Drop what `source` recorded for a module: a long-running host (a development session) that compiles many
/// sources calls it once it has read a source's records, origins and plans, so the registry stays bounded.
pub fn forget(path: &Path) {
    registry().lock().unwrap().remove(&key(path));
}

/// The kit's findings for a module read through `source`.
pub fn records(path: &Path) -> Vec<Record> {
    registry().lock().unwrap().get(&key(path)).map(|e| e.records.clone()).unwrap_or_default()
}

/// The source line of each line of a module's elaborated text (from 1).
pub fn origins(path: &Path) -> Vec<usize> {
    registry().lock().unwrap().get(&key(path)).map(|e| e.origins.clone()).unwrap_or_default()
}

/// The plans (static provenance) of a module read through `source`.
pub fn plans(path: &Path) -> Vec<Json> {
    registry().lock().unwrap().get(&key(path)).map(|e| e.plans.clone()).unwrap_or_default()
}

/// The records of modules read through `source`, as every surface reports them (the batch commands and the query
/// database alike): a module whose elaboration failed is reported by the kit's findings alone, as a module that
/// does not parse is by its syntax error; every other record found in generated code moves to the declaration it
/// was generated from.
pub fn report(recs: &mut Vec<Record>, files: &[PathBuf]) {
    report_with(recs, files, &|p: &Path| p.display().to_string());
}

/// As `report`, with the kit's records naming their files as `name` does, the way the caller's core records name
/// them (fortisc: relative to the repository, however the command named the package), so that a failed module's
/// core records are recognised and both kinds of record name a file alike (D-90). The kit's findings follow the
/// core's in one order, by file, line and column, whatever order the modules were read in.
pub fn report_with(recs: &mut Vec<Record>, files: &[PathBuf], name: &dyn Fn(&Path) -> String) {
    let mut di = Vec::new();
    for f in files {
        for mut r in records(f) {
            r.file = name(Path::new(&r.file));
            di.push(r);
        }
    }
    di.sort_by(|a, b| (a.file.as_str(), a.line, a.column).cmp(&(b.file.as_str(), b.line, b.column)));
    let failed: std::collections::BTreeSet<String> = di.iter().map(|r| r.file.clone()).collect();
    recs.retain(|r| !failed.contains(&r.file));
    for r in recs.iter_mut() {
        remap(r);
    }
    recs.extend(di);
}

/// A core record located in generated code moves to the declaration it was generated from, and keeps the
/// generated line it was found on as `generated_line`.
pub fn remap(r: &mut Record) {
    let reg = registry().lock().unwrap();
    let Some(e) = reg.get(&key(Path::new(&r.file))) else { return };
    if r.line > e.source_lines {
        if let Some(&o) = e.origins.get(r.line - 1) {
            r.fields.insert("generated_line".into(), Json::Int(r.line as i64));
            r.line = o;
            r.column = 1;
        }
    } else if let Some(&o) = e.origins.get(r.line - 1) {
        r.line = o;
    }
}
