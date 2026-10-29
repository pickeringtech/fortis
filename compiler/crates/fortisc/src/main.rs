//! fortisc: the production frontend's command (DEL-31 onwards).
//!
//!   fortisc check FILE.ft|DIR [--format json|text] [--api-baseline OLD]
//!                                         syntax, names, declarations and the typed walk; records to stdout as JSON
//!                                         lines when stdout is not a terminal, text on one (PROG-DIA-07); `--json`
//!                                         is the conformance harness's form (records grouped under `=== FILE`)
//!   fortisc build FILE.ft|DIR [--target TRIPLE] [--out PATH] [--format json|text]
//!                                         check with the target's feature set, then lower and build a native
//!                                         executable offline, with a build manifest (PATH.build.json)
//!   fortisc test FILE.ft|DIR [--test T] [--case C] [--seed S] [--cases N] [--fuzz [--budget N] [--corpus DIR]]
//!                [--replay FILE] [--format json|text]
//!                                         check, then every test, table row and property on the VM; `--fuzz`
//!                                         fuzzes each property (fortis_vm::testing); a FILE.ft that is a module
//!                                         of a package (the nearest project.yaml whose src/ holds it) is checked
//!                                         and built as that package's module, and only its tests run (D-54)
//!   fortisc elaborate FILE... [--plan]    the module as the core reads it: the Dependency injection kit's
//!                                         elaboration (SP-08); `--plan` prints each context's resolved plan
//!   fortisc syntax FILE...                the syntax runner's judgement per file
//!   fortisc parse FILE...                 each module's concrete syntax tree
//!   fortisc vm FILE...                    run main on the development VM (DEL-35). Exit is the program's:
//!                                         0 success, 1 a failing main or a refusal, 2 a dynamic check, 101 a defect.
//!                                         A file under a package's src/ runs that package's program (D-327).
//!   fortisc mir [--json] FILE...           the typed IR every backend consumes (Programme 5.2)
//!   fortisc ir FILE...                    the function IR (CFG, loans, obligations, effects, observation, cleanup)
//!   fortisc decls FILE...                 the declaration IR of each module
//!   fortisc cfg FILE...                   the control-flow graph of each block-bodied function
mod build;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fortis_diag::{Json, Record};

/// A line to standard output; a reader that has gone away (`fortisc view … | head`) ends the run quietly.
macro_rules! outln {
    ($($a:tt)*) => {{
        use std::io::Write;
        if writeln!(std::io::stdout(), $($a)*).is_err() {
            std::process::exit(0);
        }
    }};
}

/// A module as the core reads it: elaborated when it uses the Dependency injection kit's heads (fortis-di).
fn read_module(path: impl AsRef<Path>) -> String {
    fortis_di::source(path.as_ref()).unwrap_or_default()
}

/// The kit's records joined to the core's (fortis_di::report), for a module or every module of a package.
fn with_di_records(recs: &mut Vec<Record>, p: &Path) {
    let files = di_files(p);
    for f in &files {
        let _ = fortis_di::source(f);
    }
    // the kit's records name a file as the core's do: a package's modules relative to the repository
    // (fortis_names::resolve::rel), a single module as the command named it (D-139)
    if p.is_dir() {
        fortis_di::report_with(recs, &files, &|f: &Path| fortis_names::resolve::rel(f));
    } else {
        fortis_di::report_with(recs, &files, &|f: &Path| f.display().to_string());
    }
}

/// The modules whose Dependency injection findings belong to a check or a run of `p`: the module itself, or every
/// module of the package set a package directory loads, its dependencies' too, since they are checked, elaborated,
/// lowered and run with it (D-139).
fn di_files(p: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if p.is_dir() {
        let search = p.parent().unwrap_or(Path::new("."));
        match fortis_names::resolve::PackageSet::load(p, search) {
            Ok(ps) => {
                for (_, pkg) in ps.packages.iter() {
                    files.extend(pkg.modules.values().map(|m| m.file.clone()));
                }
            }
            Err(_) => ft_files(&p.join("src"), &mut files),
        }
    } else {
        files.push(p.to_path_buf());
    }
    files
}

/// The Dependency injection kit's findings for a module or for every module of a package set (`di_files`).
fn di_records(p: &Path) -> Vec<Record> {
    let mut out = Vec::new();
    for f in di_files(p) {
        let _ = fortis_di::source(&f);
        out.extend(fortis_di::records(&f));
    }
    out
}

/// The check of one file, contained (E-INT, Programme 7.1): a defect inside the checker is an `E-INT-001` record
/// with a minimised reproduction, never a crash of the batch command.
fn check_file(path: &str) -> Vec<Record> {
    let src = read_module(path);
    match contained(|| check_file_raw(path)) {
        Ok(r) => r,
        Err(message) => vec![internal_fault(path, &src, &message)],
    }
}

/// The package a file is a module of (LR 4.2): the nearest directory above it holding a `project.yaml`, when the
/// file lies under that package's `src/`; else None (a standalone module).
fn package_dir_of(f: &Path) -> Option<PathBuf> {
    let abs = std::fs::canonicalize(f).ok()?;
    let mut d = abs.parent()?;
    loop {
        if d.join("project.yaml").is_file() {
            return abs.starts_with(d.join("src")).then(|| d.to_path_buf());
        }
        d = d.parent()?;
    }
}

/// The package check's records for one module, plus manifest-level records (`project.yaml`).
fn records_for_file(recs: &[Record], want: &Option<PathBuf>, root: &Path) -> Vec<Record> {
    recs.iter()
        .filter(|r| {
            let rf = Path::new(&r.file);
            if rf.file_name().map(|n| n == "project.yaml").unwrap_or(false) {
                return true;
            }
            let abs = if rf.is_absolute() { rf.to_path_buf() } else { root.join(rf) };
            std::fs::canonicalize(abs).ok().map(|a| want.as_ref() == Some(&a)).unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// `fortisc check FILE.ft` where FILE.ft is one module of a package: the package context decides, so the file's
/// findings are the package check's findings for it, plus the manifest-level records, without which the module
/// cannot be judged at all. Checking the file alone would resolve its imports against nothing and accept an
/// import of a member the imported module does not declare (D-51.8); `fortis check` agrees (bin/fortis).
fn check_file_in_context(f: &str) -> Vec<Record> {
    let p = Path::new(f);
    let Some(dir) = package_dir_of(p) else { return check_file(f) };
    let search = dir.parent().unwrap_or(Path::new("."));
    let root = fortis_names::library::repo_root();
    let want = std::fs::canonicalize(p).ok();
    match fortis_names::resolve::PackageSet::load(&dir, search) {
        Err(e) => vec![fortis_names::resolve::manifest_record(&e, &fortis_names::resolve::rel(&dir.join("project.yaml")))],
        Ok(mut ps) => {
            let named = ps.run();
            let mut mine = records_for_file(&named, &want, &root);
            // a sibling that does not parse makes the package check return before this module's imports are checked.
            // Dropping that syntax record would accept the file. Continuing the name check would judge the unparsed
            // module as exporting nothing (D-51.8). The file is reported as not name-checked.
            if mine.is_empty() && named.iter().any(|r| r.code.starts_with("E-SYN")) {
                let manifest = fortis_names::resolve::rel(&dir.join("project.yaml"));
                mine.push(Record::new(
                    "E-NAM-001",
                    "this module was not name-checked: another module of the package does not parse",
                    "LANG-04-03",
                    &manifest,
                    1,
                    1,
                    Json::obj([("name", Json::str("")), ("candidates", Json::strs(Vec::<String>::new()))]),
                ));
            } else if mine.is_empty() {
                // a sibling's name error made `run` skip the type pass, and the filter then dropped that sibling.
                // This file is still type-checked. A clean file stays clean. The whole-package check still stops at
                // the name error (`check_package_dir`). An unparsed sibling is the case above and is not type-checked.
                mine = records_for_file(&fortis_types::check::check_package_set(&ps), &want, &root);
            }
            mine
        }
    }
}

/// Run `f`, turning a panic into its message (the default report on standard error is suppressed meanwhile).
fn contained<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    use std::sync::{Arc, Mutex};
    let message: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let m2 = message.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = info.payload().downcast_ref::<String>().cloned().or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "a panic".into());
        let at = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        *m2.lock().unwrap() = format!("{text}{at}");
    }));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(previous);
    r.map_err(|_| message.lock().unwrap().clone())
}

/// `E-INT-001` for a checker defect: the input is minimised to the smallest module that still fails the same way
/// (whole declarations, then single lines, removed while the failure persists), saved with the command that
/// reproduces it, and the record points at it.
fn internal_fault(path: &str, src: &str, message: &str) -> Record {
    let failure = message.rsplit_once(" at ").map_or(message, |x| x.0).to_string();
    let fails = |text: &str| -> bool {
        let Ok(parsed) = fortis_syntax::parse_module(text) else { return false };
        matches!(contained(|| fortis_types::check::check_parsed(&parsed, path)), Err(m) if m.rsplit_once(" at ").map_or(m.as_str(), |x| x.0) == failure)
    };
    let mut lines: Vec<String> = src.lines().map(|l| l.to_string()).collect();
    // declarations first: a declaration's lines run from its first token to the line before the next one's
    if let Ok(parsed) = fortis_syntax::parse_module(src) {
        let root = fortis_syntax::view::N::new(&parsed.root, &parsed.toks);
        let starts: Vec<usize> = root.kids().into_iter().filter_map(|c| c.node()).filter_map(|n| n.first_token().map(|t| t.line)).collect();
        let mut spans: Vec<(usize, usize)> = starts.iter().enumerate().map(|(i, s)| (*s, starts.get(i + 1).map_or(lines.len() + 1, |n| *n))).collect();
        spans.reverse();
        for (a, b) in spans {
            let trial: Vec<String> = lines.iter().enumerate().filter(|(i, _)| !(a <= i + 1 && i + 1 < b)).map(|(_, l)| l.clone()).collect();
            if trial.len() < lines.len() && fails(&trial.join("\n")) {
                lines = trial;
            }
        }
    }
    let mut i = lines.len();
    while i > 0 {
        i -= 1;
        let mut trial = lines.clone();
        trial.remove(i);
        if fails(&trial.join("\n")) {
            lines = trial;
        }
    }
    let minimal = format!("{}\n", lines.join("\n"));
    let root = fortis_names::library::repo_root();
    let digest = fortis_names::sha256::sha256(minimal.as_bytes());
    let dir = root.join("build").join("fortis-int").join(&digest[..12]);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("input.ft"), &minimal);
    let fault = std::env::var("FORTIS_FAULT").map(|v| format!("FORTIS_FAULT={v} ")).unwrap_or_default();
    let _ = std::fs::write(
        dir.join("README"),
        format!("{fault}fortisc check input.ft\n\nfailure: {message}\noriginal: {path} ({} lines; minimal: {} lines)\ncompiler: {}\n", src.lines().count(), lines.len(), fortis_names::lock::compiler_digest()),
    );
    internal_defect(path, 1, &format!("the checker failed ({failure}); minimal reproduction: {} lines of {}", lines.len(), src.lines().count()), &dir.display().to_string())
}

fn check_file_raw(path: &str) -> Vec<Record> {
    let src = match fortis_di::source(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{path}: {e}");
            return vec![];
        }
    };
    let o = fortis_syntax::check::judge(path, &src);
    if !o.ok {
        return o.records;
    }
    if o.form != "module" {
        return vec![];
    }
    let parsed = o.parsed.unwrap();
    // `--units`: the same result computed unit by unit (the query database's decomposition, DEL-46)
    if std::env::args().any(|a| a == "--units") {
        return fortis_types::incremental::from_scratch(&parsed, path, "main");
    }
    fortis_types::check::check_parsed(&parsed, path)
}

/// A file is one module (`main`); a directory is a package (project.yaml and src/), every module built.
fn build_target(f: &str, src: &str, lib: &fortis_mir::build::LibIndex) -> Result<fortis_mir::model::Program, (Vec<fortis_diag::Record>, Vec<fortis_mir::build::BuildError>)> {
    let p = Path::new(f);
    // a module or package set whose Dependency injection elaboration failed is never lowered: it is refused on the
    // kit's findings, as `check` refuses it (D-139)
    let di = di_records(p);
    if !di.is_empty() {
        return Err((di, vec![]));
    }
    if p.is_dir() {
        let search = p.parent().unwrap_or(Path::new("."));
        fortis_mir::build_package(p, search, lib)
    } else {
        fortis_mir::build_source(src, f, lib)
    }
}

/// The package a module file belongs to: the nearest ancestor directory holding a project.yaml, when its src/ holds
/// the file; the package's directory and the module's dotted path. A file the nearest package's src/ does not hold
/// is no module of it and stays standalone.
fn enclosing_package(f: &Path) -> Option<(std::path::PathBuf, String)> {
    let abs = std::fs::canonicalize(f).ok()?;
    let mut dir = abs.parent()?;
    loop {
        if dir.join("project.yaml").is_file() {
            let rel = abs.strip_prefix(dir.join("src")).ok()?;
            if rel.extension().map_or(false, |e| e == "ft") {
                let module = rel.with_extension("").components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join(".");
                return Some((dir.to_path_buf(), module));
            }
            return None;
        }
        dir = dir.parent()?;
    }
}

/// The unit `run`, `vm`, `build` and `native` judge. A file under a package's `src/` is that package (LR 4.2), as
/// `check` and `test` already judge it (D-327). A directory is itself. A file the nearest package's `src/` does not
/// hold stays a standalone module.
struct RunUnit {
    root: PathBuf,
    /// set when the user named a module file of `root`
    module: Option<String>,
}

fn run_unit(f: &str) -> RunUnit {
    let p = Path::new(f);
    if !p.is_dir() {
        if let Some((dir, module)) = enclosing_package(p) {
            return RunUnit { root: dir, module: Some(module) };
        }
    }
    RunUnit { root: p.to_path_buf(), module: None }
}

/// The exit status of several programs: a defect (101) outranks a dynamic check (2), which outranks a failing main
/// or a refusal (1).
fn worse_exit(a: i32, b: i32) -> i32 {
    fn rank(c: i32) -> i32 {
        match c {
            101 => 4,
            2 => 3,
            1 => 2,
            0 => 0,
            c if c > 0 => 1,
            _ => 0,
        }
    }
    if rank(b) > rank(a) { b } else { a }
}

/// `os.path.abspath`: join with the cwd and collapse `.` and `..`, without resolving symlinks.
fn python_abspath(p: &Path) -> PathBuf {
    let joined = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p) };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from(".") } else { out }
}

fn remember_decl(seen: &mut std::collections::HashSet<String>, pairs: &mut Vec<(String, String)>, name: String, file: &str) {
    if name.is_empty() || !seen.insert(name.clone()) {
        return;
    }
    pairs.push((name, file.to_string()));
}

/// How a runtime record names a module's file, on every backend and in a native executable (D-302): a module of a
/// package relative to the package's directory, and a standalone module relative to the current directory when it
/// lies below it (the corpus runs from the repository root), else by its file name. Never absolute, so where a
/// package lies does not reach its executable, two checkouts build the same one, and no user's home path is built
/// into it. The interpreter's `shown_path` (interpreter/records.py) is the same.
fn shown_path(file: &Path, package: Option<&Path>) -> String {
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| python_abspath(p));
    let f = real(file);
    let shown = match package {
        // `os.path.relpath`: a dependency's module lies beside the package, under `..`
        Some(dir) => {
            let d = real(dir);
            let (fc, dc): (Vec<_>, Vec<_>) = (f.components().collect(), d.components().collect());
            let common = fc.iter().zip(&dc).take_while(|(a, b)| a == b).count();
            std::iter::repeat(std::path::Component::ParentDir).take(dc.len() - common).chain(fc[common..].iter().copied()).collect::<PathBuf>()
        }
        None => f.strip_prefix(real(Path::new("."))).map(Path::to_path_buf).unwrap_or_else(|_| f.file_name().map(PathBuf::from).unwrap_or_default()),
    };
    if shown.as_os_str().is_empty() { ".".into() } else { shown.display().to_string() }
}

/// The file a failure names, matching the interpreter, as `shown_path` gives it. A standalone module names its own
/// file. A package names the entry module's file, and a declaration of any other module names that module's file.
/// The first declaration of a source name wins (`decl_file.setdefault`). The entry module stays out of the map, so
/// its failures use the fallback, including when the user named the package directory or a file under `src/`.
fn report_files(user: &str) -> (String, Vec<(String, String)>) {
    let unit = run_unit(user);
    let root = unit.root.as_path();
    if !root.join("project.yaml").is_file() {
        return (shown_path(Path::new(user), None), vec![]);
    }
    // A directory keeps Python's abspath. A file under src/ keeps the canonical package directory
    // `enclosing_package` already computed, which is Python's realpath.
    let root_dir = if Path::new(user).is_dir() { python_abspath(Path::new(user)) } else { root.to_path_buf() };
    let search = root_dir.parent().unwrap_or(Path::new(".")).to_path_buf();
    let ps = match fortis_names::resolve::PackageSet::load(&root_dir, &search) {
        Ok(ps) => ps,
        Err(_) => return (shown_path(Path::new(user), Some(&root_dir)), vec![]),
    };
    let entry_module = ps.packages.first().and_then(|(_, p)| p.entry.clone());
    let fallback = entry_module.as_ref().and_then(|m| ps.packages.first().and_then(|(_, p)| p.modules.get(m).map(|module| shown_path(&module.file, Some(&root_dir))))).unwrap_or_else(|| shown_path(Path::new(user), Some(&root_dir)));
    let mut seen = std::collections::HashSet::new();
    let mut pairs = Vec::new();
    for (i, (_, pkg)) in ps.packages.iter().enumerate() {
        for (mpath, module) in &pkg.modules {
            if i == 0 && entry_module.as_deref() == Some(mpath.as_str()) {
                continue;
            }
            let file = shown_path(&module.file, Some(&root_dir));
            for d in module.decls() {
                if let Some(name) = d.name.clone() {
                    remember_decl(&mut seen, &mut pairs, name, &file);
                }
                if d.kind == "class" || d.kind == "enum" {
                    for member in d.node.kids() {
                        let Some(n) = member.node() else { continue };
                        if n.name() != "classMember" && n.name() != "enumMember" {
                            continue;
                        }
                        let Some(head) = n.find("methodHead") else { continue };
                        let Some(mn) = head.find("memberName") else { continue };
                        let Some(tok) = mn.tokens().into_iter().next() else { continue };
                        remember_decl(&mut seen, &mut pairs, tok.text.clone(), &file);
                    }
                }
            }
        }
    }
    (fallback, pairs)
}

/// A module of a package under a test run (D-54): the whole package is built, so the module's imports resolve, and
/// the program holds that module's tests and no other module's.
fn build_package_module_target(dir: &Path, module: &str, lib: &fortis_mir::build::LibIndex) -> Result<fortis_mir::model::Program, (Vec<fortis_diag::Record>, Vec<fortis_mir::build::BuildError>)> {
    let di = di_records(dir);
    if !di.is_empty() {
        return Err((di, vec![]));
    }
    fortis_mir::build_package_module(dir, dir.parent().unwrap_or(Path::new(".")), lib, module)
}

fn write_if_changed(path: &Path, content: &str) {
    if std::fs::read_to_string(path).map_or(true, |old| old != content) {
        let _ = std::fs::write(path, content);
    }
}

/// A program's name for its generated crate and executable: a file's stem, or a package directory's whole name
/// (`acme.main.async` is not `acme.main`, so two packages never share a crate).
fn program_stem(p: &Path) -> String {
    if p.is_dir() {
        p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    }
}

/// `E-NAM-030` for every module of a file or package that imports a standard module the target lacks.
fn target_records(p: &Path, target: &str) -> Vec<Record> {
    let mut files = Vec::new();
    if p.is_dir() {
        ft_files(&p.join("src"), &mut files);
    } else {
        files.push(p.to_path_buf());
    }
    let mut out = Vec::new();
    for f in files {
        let src = read_module(&f);
        if let Ok(parsed) = fortis_syntax::parse_module(&src) {
            out.extend(fortis_names::features::check_imports(&parsed, &f.display().to_string(), target));
        }
    }
    out
}

fn sha_file(p: &Path) -> String {
    fortis_names::sha256::sha256(&std::fs::read(p).unwrap_or_default())
}

/// Write `content` unless the file already holds it; true when written (PROG-NFR-07).
fn write_changed(path: &Path, content: &str) -> bool {
    if std::fs::read_to_string(path).map_or(true, |old| old != content) {
        let _ = std::fs::write(path, content);
        true
    } else {
        false
    }
}

/// `fortisc build` (DEL-43): see build.rs.
fn run_build(files: &[&String], flag: &dyn Fn(&str) -> Option<String>, as_json: bool) -> ExitCode {
    use std::time::Instant;
    let root = fortis_names::library::repo_root();
    let host = fortis_names::features::host_target();
    let target = flag("--target").unwrap_or_else(|| host.to_string());
    let features = fortis_names::features::features(&target).unwrap_or_default();
    let lib = fortis_mir::build::LibIndex::load(&root);
    let core = fortis_mir::library_program(&root, &lib);
    let work = root.join("build").join("native-build");
    let mut bad = false;
    for f in files {
        let t0 = Instant::now();
        // a module file under a package's src/ is that package (D-327): the package check, then the package program
        let unit = run_unit(f);
        let p = unit.root.as_path();
        // 1. the check, with the target's feature set: a module the target lacks fails here, before any native step
        let mut recs = if p.is_dir() { fortis_types::check::check_package_dir(p, p.parent().unwrap_or(Path::new("."))) } else { check_file(f) };
        with_di_records(&mut recs, p); // the kit's findings, as `check` reports them (D-139)
        recs.extend(target_records(p, &target));
        // plan compatibility (E-EFF-030), the build-time step check does not run (PROG-VM-04)
        if !recs.iter().any(|r| r.severity == "error") {
            recs.extend(fortis_types::plans::check_path(p));
        }
        // 2. a package that ships a build script is refused (PROG-CMP-06)
        if p.is_dir() {
            let dirs: Vec<std::path::PathBuf> = match fortis_names::resolve::PackageSet::load(p, p.parent().unwrap_or(Path::new("."))) {
                Ok(ps) => ps.packages.iter().map(|(_, q)| q.dir.clone()).collect(),
                Err(_) => vec![p.to_path_buf()],
            };
            recs.extend(build::build_scripts(&dirs).iter().map(|(pkg, script)| build::build_script_record(pkg, script)));
            // a native build also holds the Rust toolchain to the lockfile (check holds the rest, DEL-42)
            if let Ok(text) = std::fs::read_to_string(p.join(fortis_names::lock::FILE)) {
                if let Ok(lock) = fortis_names::lock::Lock::parse(&text) {
                    recs.extend(lock.verify(p, p.parent().unwrap_or(Path::new(".")), fortis_names::lock::Pins::All).into_iter().filter(|r| r.message.contains("Rust toolchain")));
                }
            }
        }
        emit_records(&recs, as_json);
        if recs.iter().any(|r| r.severity == "error") {
            bad = true;
            continue;
        }
        if target != host {
            eprintln!("{f}: checked for {target}; building for it needs that target's linker and SDK, which this host does not declare");
            bad = true;
            continue;
        }
        let src = if p.is_dir() { String::new() } else { read_module(p) };
        let build_at = if p.is_dir() { p.to_string_lossy().into_owned() } else { f.to_string() };
        let prog = match build_target(&build_at, &src, &lib) {
            Ok(prog) => fortis_mir::link(prog, &core),
            Err((recs, errs)) => {
                emit_records(&recs, as_json);
                for e in errs {
                    eprintln!("{f}:{}: not built: {}", e.line, e.message);
                }
                bad = true;
                continue;
            }
        };
        if let Some(v) = fortis_validate::validate(&prog).into_iter().find(|v| user_violation(v)) {
            emit_records(&[internal_defect(f, v.line as usize, &format!("the typed IR is invalid ({} in {}): {}", v.check, v.function, v.message), "")], as_json);
            bad = true;
            continue;
        }
        // the instantiation budget (E-LIM-001): a generic function instantiating itself at ever larger types has no
        // last instantiation, so the build stops here rather than hand the backend an unbounded program
        if let Err(x) = fortis_validate::limits::instantiations(&prog) {
            let r = Record::new(
                "E-LIM-001",
                format!(
                    "the build's budget of {} is exhausted in {}: a generic function instantiates itself at an ever larger type, so it has no last instantiation; recurse through a non-generic helper over the concrete type, or pass a `dynamic` interface value",
                    x.budget,
                    x.function.rsplit("::").next().unwrap_or(&x.function)
                ),
                "PROG-SEC-01",
                f,
                x.line as usize,
                1,
                fortis_diag::Json::obj([("budget", fortis_diag::Json::str(&x.budget)), ("operation", fortis_diag::Json::str(&format!("instantiating {}", x.chain.join(" -> "))))]),
            );
            emit_records(&[r], as_json);
            bad = true;
            continue;
        }
        let frontend_ms = t0.elapsed().as_secs_f64() * 1e3;
        // 3. lower: the generated crate, rewriting only the files that changed
        let stem = program_stem(p);
        let crate_name = format!("fb_{}", stem.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect::<String>());
        // the generated Cargo.toml (a reproduction aid: `cargo build` in its directory) names the runtime relatively
        let (report_file, decl_files) = report_files(f);
        let opts = fortis_lower::Options { runtime: "../../../compiler/crates/fortis-native".into(), crate_name: crate_name.clone(), file: report_file, decl_files, tests: false };
        let t1 = Instant::now();
        let lowered = match fortis_lower::lower(&prog, &opts) {
            Ok(l) => l,
            Err(es) => {
                let e = &es[0];
                emit_records(&[internal_defect(f, e.line as usize, &format!("the lowering has no shape for {} ({})", e.function, e.message), "")], as_json);
                bad = true;
                continue;
            }
        };
        let lower_ms = t1.elapsed().as_secs_f64() * 1e3;
        let dir = work.join(&crate_name);
        let _ = std::fs::create_dir_all(dir.join("src"));
        let mut rewritten = Vec::new();
        for (name, content) in [("Cargo.toml", &lowered.cargo_toml), ("src/main.rs", &lowered.main_rs)] {
            if write_changed(&dir.join(name), content) {
                rewritten.push(name.to_string());
            }
        }
        // 4. the runtime crates, vendored and prebuilt (their graph holds no build script and no registry crate)
        let (rlib, deps) = match build::prebuilt_runtime(&root) {
            Ok(x) => x,
            Err(build::RuntimeError::Graph(e)) => {
                emit_records(&[build::build_script_record(&root.join("compiler").join("crates").join("fortis-native"), Path::new(&e))], as_json);
                bad = true;
                continue;
            }
            Err(build::RuntimeError::Tool(e)) => {
                emit_records(&[build::toolchain_record(f, &e)], as_json);
                bad = true;
                continue;
            }
        };
        // 5. compile and link with rustc, the link timed by this executable acting as the linker; an unchanged
        // program against an unchanged runtime is not compiled again
        let exe_self = std::env::current_exe().unwrap_or_default();
        let built = dir.join(&crate_name);
        // rustc writes a staging file. A success that does not create it must not refresh the stamp or publish
        // the previous `built` (D-55). The staging name is constant, so it does not churn the stamp by itself
        // beyond the one-time change of `-o`.
        let staged = dir.join(format!("{crate_name}.staged"));
        let rustc_args = build::rustc_args(&root, &crate_name, &dir.join("src").join("main.rs"), &staged, &rlib, &deps, &exe_self);
        let stamp_want = fortis_names::sha256::sha256(format!("{}\n{}\n{}", sha_file(&dir.join("src").join("main.rs")), sha_file(&rlib), rustc_args.join(" ")).as_bytes());
        let stamp = dir.join("stamp");
        let times = dir.join("link-times.txt");
        let _ = std::fs::remove_file(&times);
        let t2 = Instant::now();
        if !(built.exists() && std::fs::read_to_string(&stamp).map_or(false, |s| s.trim() == stamp_want)) {
            let _ = std::fs::remove_file(&staged);
            let out = std::process::Command::new("rustc").args(&rustc_args).current_dir(&dir).env("FORTIS_LINK_TIMES", &times).output();
            match out {
                Ok(o) if o.status.success() => match std::fs::metadata(&staged) {
                    Ok(m) if m.len() > 0 => {
                        if let Err(e) = std::fs::rename(&staged, &built) {
                            emit_records(&[backend_error(f, &dir, &format!("rustc wrote an executable but it could not replace the previous one ({e})"), &rustc_args)], as_json);
                            bad = true;
                            continue;
                        }
                        let _ = std::fs::write(&stamp, &stamp_want);
                    }
                    Ok(_) => {
                        let _ = std::fs::remove_file(&staged);
                        emit_records(&[backend_error(f, &dir, "rustc reported success but the executable it wrote is empty", &rustc_args)], as_json);
                        bad = true;
                        continue;
                    }
                    Err(e) => {
                        emit_records(&[backend_error(f, &dir, &format!("rustc reported success but wrote no executable ({e})"), &rustc_args)], as_json);
                        bad = true;
                        continue;
                    }
                },
                Ok(o) => {
                    let _ = std::fs::remove_file(&staged);
                    emit_records(&[backend_error(f, &dir, &String::from_utf8_lossy(&o.stderr), &rustc_args)], as_json);
                    bad = true;
                    continue;
                }
                Err(e) => {
                    emit_records(&[build::toolchain_record(f, &format!("rustc: {e}"))], as_json);
                    bad = true;
                    continue;
                }
            }
        }
        // the executable that will be published is the one rustc's staging file replaced, never a stale binary
        // left behind a new stamp (D-55)
        match std::fs::metadata(&built) {
            Ok(m) if m.len() > 0 => {}
            Ok(_) => {
                emit_records(&[backend_error(f, &dir, "rustc reported success but the executable it wrote is empty", &rustc_args)], as_json);
                bad = true;
                continue;
            }
            Err(e) => {
                emit_records(&[backend_error(f, &dir, &format!("rustc reported success but wrote no executable ({e})"), &rustc_args)], as_json);
                bad = true;
                continue;
            }
        }
        let cargo_ms = t2.elapsed().as_secs_f64() * 1e3;
        let link_ms: f64 = std::fs::read_to_string(&times).unwrap_or_default().lines().filter_map(|l| l.trim().parse::<f64>().ok()).fold(0.0, |a, b| a + b);
        // 6. the executable and its manifest
        let out_path = flag("--out").map(std::path::PathBuf::from).unwrap_or_else(|| root.join("build").join("bin").join(&stem));
        if let Some(d) = out_path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let bytes = match std::fs::read(&built) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => {
                emit_records(&[backend_error(f, &dir, "rustc reported success but the executable it wrote is empty", &rustc_args)], as_json);
                bad = true;
                continue;
            }
            Err(e) => {
                emit_records(&[backend_error(f, &dir, &format!("the executable rustc wrote could not be read ({e})"), &rustc_args)], as_json);
                bad = true;
                continue;
            }
        };
        // a failure to publish is a failed build: it exits non-zero and leaves the previous executable in
        // place. The bytes land in a sibling temporary and replace `out` only once they are runnable (D-55).
        if let Err(e) = build::publish_executable(&out_path, &bytes) {
            emit_records(&[internal_defect(f, 1, &e, "")], as_json);
            bad = true;
            continue;
        }
        let mut inputs = Vec::new();
        let mut sources = Vec::new();
        if p.is_dir() {
            sources.push(p.join("project.yaml"));
            ft_files(&p.join("src"), &mut sources);
            if p.join("fortis.lock").exists() {
                sources.push(p.join("fortis.lock"));
            }
        } else {
            sources.push(p.to_path_buf());
        }
        for s in &sources {
            inputs.push((fortis_names::resolve::rel(s), sha_file(s)));
        }
        let mut effects = std::collections::BTreeMap::new();
        for func in prog.functions.iter().filter(|x| fortis_mir::model::is_user_module(&x.module) && x.body.is_some()) {
            effects.insert(func.name.clone(), (func.effects.effects.iter().map(|(e, _)| e.clone()).collect(), func.effects.authorities.iter().cloned().collect()));
        }
        let manifest = build::Manifest {
            program: f.to_string(),
            target: target.clone(),
            features: features.clone(),
            inputs,
            generated: vec![("Cargo.toml".into(), sha_file(&dir.join("Cargo.toml"))), ("src/main.rs".into(), sha_file(&dir.join("src").join("main.rs")))],
            rewritten,
            binary: Some((fortis_names::sha256::sha256(&bytes), bytes.len() as u64, build::elf_sections(&bytes))),
            allocation: build::allocation(&prog, &lowered.audit),
            effects,
            trust: build::trust(&prog),
            timings: vec![("frontend", frontend_ms), ("lower", lower_ms), ("compile", (cargo_ms - link_ms).max(0.0)), ("link", link_ms)],
            compiler: fortis_names::lock::compiler_digest(),
            stdlib: fortis_names::lock::stdlib_digest(),
        };
        let mpath = std::path::PathBuf::from(format!("{}.build.json", out_path.display()));
        let previous = std::fs::read_to_string(&mpath).ok().and_then(|t| fortis_diag::Json::parse(&t).ok());
        let j = manifest.to_json(previous.as_ref());
        let _ = std::fs::write(&mpath, format!("{}\n", j.to_line()));
        if as_json {
            outln!("{}", fortis_diag::Json::obj([("kind", fortis_diag::Json::str("build")), ("binary", fortis_diag::Json::str(&out_path.display().to_string())), ("manifest", fortis_diag::Json::str(&mpath.display().to_string()))]).to_line());
        } else {
            let size = bytes.len();
            let delta = j.get("changes").and_then(|c| c.get("code_size")).and_then(|c| c.get("bytes")).map(|d| format!(" ({}{} bytes)", if matches!(d, fortis_diag::Json::Int(i) if *i >= 0) { "+" } else { "" }, d.to_line())).unwrap_or_default();
            outln!("built {} for {target}: {size} bytes{delta}; compile {:.0} ms, link {:.0} ms; manifest {}", out_path.display(), (cargo_ms - link_ms).max(0.0), link_ms, mpath.display());
        }
    }
    if bad {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// An internal defect (`E-INT-001`): the compiler failed in a way no source program should cause.
fn internal_defect(file: &str, line: usize, what: &str, reproduction: &str) -> Record {
    Record::new(
        "E-INT-001",
        format!("internal compiler defect: {what}"),
        "PROG-CMP-05",
        file,
        line.max(1),
        1,
        fortis_diag::Json::obj([("reproduction", fortis_diag::Json::str(reproduction))]),
    )
}

/// A backend error (`E-INT-002`, PROG-CMP-05): rustc rejected the generated Rust, which is always a compiler defect
/// in the Rust-checked fragment; the error is mapped to the Fortis function it came from by the generated markers.
fn backend_error(file: &str, crate_dir: &Path, stderr: &str, rustc_args: &[String]) -> Record {
    let main_rs = std::fs::read_to_string(crate_dir.join("src").join("main.rs")).unwrap_or_default();
    let lines: Vec<&str> = main_rs.lines().collect();
    let first = stderr.lines().find(|l| l.starts_with("error")).unwrap_or("rustc failed").to_string();
    let at = stderr.lines().filter(|l| l.trim().starts_with("--> ")).find_map(|l| l.split("src/main.rs:").nth(1)).and_then(|r| r.split(':').next()).and_then(|n| n.parse::<usize>().ok());
    let origin = at.and_then(|n| lines.iter().take(n).rev().find_map(|l| l.trim().strip_prefix("// fortis: "))).map(|s| s.to_string());
    let (function, line) = match origin.as_deref().and_then(|o| o.rsplit_once(" line ")) {
        Some((func, ln)) => (func.to_string(), ln.parse().unwrap_or(1)),
        None => (String::new(), 1),
    };
    let repro = format!("in {}: rustc {}{}", crate_dir.display(), rustc_args.join(" "), at.map(|n| format!(" (src/main.rs:{n})")).unwrap_or_default());
    // in a package, the function's module names its file (`pkg.mod::label` is `src/mod.ft`)
    let dir = Path::new(file);
    let source = if dir.is_dir() && !function.is_empty() {
        let pkg = std::fs::read_to_string(dir.join("project.yaml")).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("name:").map(|n| n.trim().to_string()))).unwrap_or_default();
        let module = function.split("::").next().unwrap_or("");
        module.strip_prefix(&format!("{pkg}.")).map(|m| dir.join("src").join(format!("{}.ft", m.replace('.', "/"))).display().to_string()).unwrap_or_else(|| file.to_string())
    } else {
        file.to_string()
    };
    let file = source.as_str();
    Record::new(
        "E-INT-002",
        if function.is_empty() { format!("internal compiler defect: the generated Rust does not compile ({first})") } else { format!("internal compiler defect: the Rust generated for {function} does not compile ({first})") },
        "PROG-CMP-05",
        file,
        line,
        1,
        fortis_diag::Json::obj([("reproduction", fortis_diag::Json::str(&repro))]),
    )
}

/// Records as JSON lines or as text (PROG-DIA-07): the format follows `--format`, else whether standard output is a
/// terminal; the text is rendered from the record.
fn json_output(args: &[String]) -> bool {
    use std::io::IsTerminal;
    match args.iter().position(|a| a == "--format").and_then(|i| args.get(i + 1)).map(|s| s.as_str()) {
        Some("json") => true,
        Some("text") => false,
        _ => !std::io::stdout().is_terminal(),
    }
}

fn emit_records(recs: &[Record], json: bool) {
    for r in recs {
        if json {
            outln!("{}", r.to_json_line());
        } else {
            outln!("{}", r.render_text());
        }
    }
}

/// A runtime record's text (interpreter/records.py `render`, extended with the test runner's records).
fn render_runtime(r: &fortis_diag::Json) -> String {
    use fortis_diag::Json;
    let s = |k: &str| match r.get(k) {
        Some(Json::Str(x)) => x.clone(),
        Some(other) => other.to_line(),
        None => String::new(),
    };
    match s("kind").as_str() {
        "test-result" => {
            let case = if s("case").is_empty() { String::new() } else { format!(" / {}", s("case")) };
            let detail = match (r.get("evidence").and_then(|e| e.get("actual")), r.get("evidence").and_then(|e| e.get("expected"))) {
                (Some(Json::Str(a)), Some(Json::Str(e))) => format!(": expected {e}, actual {a}"),
                _ => String::new(),
            };
            format!("{} {}{case}{detail}", s("status").to_uppercase(), s("test"))
        }
        "property-failure" => format!(
            "  minimal failing input ({} seed {}, {} shrinks from {}): {}\n  replay: {}",
            s("mode"),
            s("seed"),
            s("shrinks"),
            s("original"),
            s("minimal"),
            s("replay")
        ),
        // as the reference renderer writes it (interpreter/records.py)
        "log" => format!("{}: {}", s("level"), s("message")),
        "runtime-failure" => {
            let text = |j: Option<&Json>| match j {
                Some(Json::Str(x)) => x.clone(),
                Some(other) => other.to_line(),
                None => String::new(),
            };
            let at = text(r.get("origin").and_then(|o| o.get("function")));
            let pay = match r.get("payload") {
                Some(Json::Obj(fields)) => fields.iter().map(|(k, v)| format!("{k}: {}", text(Some(v)))).collect::<Vec<_>>().join(", "),
                _ => String::new(),
            };
            let pay = if pay.is_empty() { pay } else { format!(" ({pay})") };
            let message = if s("message").is_empty() { String::new() } else { format!(": {}", s("message")) };
            format!("failure {} in {at}{pay}{message}", s("code"))
        }
        _ => r.to_line(),
    }
}

/// The API changes from `old` to `new` (LANG-11-05): two files, or two package directories compared module by
/// module (each `src/` file by its path).
fn api_changes(old: &Path, new: &Path) -> Vec<Record> {
    let api = |p: &Path| -> std::collections::BTreeMap<String, fortis_types::api::ApiFn> {
        let src = read_module(p);
        match fortis_syntax::parse_module(&src) {
            Ok(parsed) => fortis_types::api::exported(&parsed, &p.display().to_string()),
            Err(_) => Default::default(),
        }
    };
    if new.is_dir() {
        let mut files = Vec::new();
        ft_files(&new.join("src"), &mut files);
        let mut out = Vec::new();
        for f in files {
            let rel = f.strip_prefix(new).unwrap_or(&f).to_path_buf();
            let before = old.join(&rel);
            if before.is_file() {
                out.extend(fortis_types::api::compare(&api(&before), &api(&f), &f.display().to_string()));
            }
        }
        out
    } else {
        fortis_types::api::compare(&api(old), &api(new), &new.display().to_string())
    }
}

/// A violation in the user's program (not in the core library, which the library run validates).
/// The identities of the functions an (elaborated) single-module source declares generic, from its parse: a function
/// with type parameters, and every method of a class, mixin or interface with type parameters or of its own
/// (Platform 11.2: a generic body never reloads compatibly, whether or not a type parameter survives in its IR).
fn declared_generic(src: &str) -> std::collections::BTreeSet<String> {
    use fortis_syntax::view::{C, N};
    let mut out = std::collections::BTreeSet::new();
    let parsed = match fortis_syntax::parse_module(src) {
        Ok(p) => p,
        Err(_) => return out,
    };
    let root = N::new(&parsed.root, &parsed.toks);
    let direct_token = |n: N, kind: fortis_syntax::lexer::Kind| -> Option<String> {
        n.kids().into_iter().find_map(|c| match c {
            C::T(t) if t.kind == kind => Some(t.text.clone()),
            _ => None,
        })
    };
    // a function's name and whether it declares type parameters of its own
    let function = |f: N| -> Option<(String, bool)> {
        let head = f.child_node("funcHead")?;
        let name = direct_token(head, fortis_syntax::lexer::Kind::Lower)?;
        Some((name, head.child_node("funcSignature").and_then(|sig| sig.child_node("typeParams")).is_some()))
    };
    for decl in root.child_nodes("declaration") {
        for node in decl.kids().into_iter().filter_map(|c| c.node()) {
            if node.name() == "funcDecl" {
                if let Some((name, true)) = function(node) {
                    out.insert(format!("main::function {name}"));
                }
            } else if node.name().ends_with("Decl") {
                let owner = match direct_token(node, fortis_syntax::lexer::Kind::Upper) {
                    Some(o) => o,
                    None => continue,
                };
                let generic = node.child_node("typeParams").is_some();
                for m in node.find_all("funcDecl") {
                    if let Some((name, own)) = function(m) {
                        if generic || own {
                            out.insert(format!("main::method {owner}.{name}"));
                        }
                    }
                }
            }
        }
    }
    out
}

fn user_violation(v: &fortis_validate::Violation) -> bool {
    let f = v.function.strip_prefix("type ").unwrap_or(&v.function);
    fortis_mir::model::is_user_module(f.split("::").next().unwrap_or(""))
}

fn main() -> ExitCode {
    // invoked by rustc as the linker of `fortisc build`: time the link (build::link_shim)
    if let Some(code) = build::link_shim() {
        return ExitCode::from(code as u8);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    // flags that take a value: their value is not a file
    let valued = ["--corrupt", "--runs", "--query", "--symbol", "--search", "--api-baseline", "--format", "--test", "--case", "--seed", "--cases", "--budget", "--corpus", "--replay", "--target", "--out", "--script", "--events", "--metrics", "--frames"];
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let as_json = json_output(&args);
    let files: Vec<&String> = args.iter().enumerate().skip(1).filter(|(i, a)| !a.starts_with("--") && !valued.contains(&args[i - 1].as_str())).map(|(_, a)| a).collect();
    // an input that does not exist is an invalid invocation (Programme 9.2: exit status 2), never an empty program
    // that checks clean or builds a binary that does nothing (D-54)
    let file_commands = ["check", "build", "test", "vm", "native", "mir", "ir", "decls", "cfg", "parse", "syntax", "elaborate", "summaries", "validate", "fmt"];
    if let Some(cmd) = args.first().filter(|c| file_commands.contains(&c.as_str())) {
        if let Some(missing) = files.iter().find(|f| !Path::new(f.as_str()).exists()) {
            eprintln!("fortisc {cmd}: {missing}: no such file or directory");
            return ExitCode::from(2);
        }
    }
    match args.first().map(|s| s.as_str()) {
        Some("check") => {
            let mut bad = false;
            let baseline = args.iter().position(|a| a == "--api-baseline").and_then(|i| args.get(i + 1)).cloned();
            for f in files {
                let p = Path::new(f);
                let mut recs = if p.is_dir() {
                    let search = p.parent().unwrap_or(Path::new("."));
                    fortis_types::check::check_package_dir(p, search)
                } else {
                    check_file_in_context(f)
                };
                with_di_records(&mut recs, p);
                // `--api-baseline OLD`: the previous version of this module or package; weakened exported
                // modifiers are reported (LANG-11-05)
                if let Some(old) = &baseline {
                    recs.extend(api_changes(Path::new(old), p));
                }
                // `--target T`: the modules the target's feature set lacks (PLAT-01-03)
                if let Some(t) = flag("--target") {
                    recs.extend(target_records(p, &t));
                }
                if json {
                    outln!("=== {f}");
                }
                emit_records(&recs, json || as_json);
                bad |= recs.iter().any(|r| r.severity == "error");
            }
            if bad {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Some("build") => run_build(&files, &flag, as_json),
        // `daemon DIR`: the incremental daemon over a package (SP-12), JSON lines on standard input and output
        Some("daemon") => {
            // the daemon is its own executable beside this one, so that only it runs on the counting allocator (X-141)
            use std::os::unix::process::CommandExt;
            let Some(dir) = files.first() else {
                eprintln!("usage: fortisc daemon DIR");
                return ExitCode::FAILURE;
            };
            let daemon = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("fortisd")));
            match daemon {
                Some(d) if d.exists() => {
                    let e = std::process::Command::new(&d).arg(dir.as_str()).exec();
                    eprintln!("daemon: cannot run {}: {e}", d.display());
                    ExitCode::FAILURE
                }
                _ => {
                    eprintln!("daemon: fortisd is not installed beside fortisc");
                    ExitCode::FAILURE
                }
            }
        }
        // `sections FILE`: each ELF section's SHA-256, for comparing two builds section by section (PROG-NFR-22)
        Some("sections") => {
            for f in &files {
                let bytes = std::fs::read(f.as_str()).unwrap_or_default();
                let d = build::elf_section_digests(&bytes);
                outln!("{}", fortis_diag::Json::Obj(d.into_iter().map(|(k, v)| (k, fortis_diag::Json::str(&v))).collect()).to_line());
            }
            ExitCode::SUCCESS
        }
        Some("test") => {
            let root = fortis_names::library::repo_root();
            let lib = fortis_mir::build::LibIndex::load(&root);
            let core = fortis_mir::library_program(&root, &lib);
            let mut bad = false;
            for f in files {
                let p = Path::new(f);
                // a file that is a module of a package (D-54): the enclosing package is loaded, checked and built,
                // and this module's tests run with its imports resolved, as testing the package directory does
                let module = if p.is_dir() { None } else { enclosing_package(p) };
                let pkg_dir = module.as_ref().map(|(d, _)| d.as_path()).or_else(|| p.is_dir().then_some(p));
                // the check comes first, with exactly the records `check` gives (PROG-DIA-07)
                let mut recs = match pkg_dir {
                    Some(d) => fortis_types::check::check_package_dir(d, d.parent().unwrap_or(Path::new("."))),
                    None => check_file(f),
                };
                with_di_records(&mut recs, pkg_dir.unwrap_or(p));
                emit_records(&recs, as_json);
                if recs.iter().any(|r| r.severity == "error") {
                    bad = true;
                    continue;
                }
                let plan = fortis_types::plans::check_path(pkg_dir.unwrap_or(p));
                if !plan.is_empty() {
                    emit_records(&plan, as_json);
                    bad = true;
                    continue;
                }
                let src = if p.is_dir() { String::new() } else { read_module(f) };
                // a statement fragment (a documentation example) is checked as a block and declares no tests
                if !p.is_dir() && fortis_syntax::check::judge(f, &src).form != "module" {
                    continue;
                }
                let prog = match match &module {
                    Some((d, m)) => build_package_module_target(d, m, &lib),
                    None => build_target(f, &src, &lib),
                } {
                    Ok(prog) => fortis_mir::link(prog, &core),
                    Err((recs, errs)) => {
                        emit_records(&recs, as_json);
                        for e in errs {
                            eprintln!("{f}:{}: not built: {}", e.line, e.message);
                        }
                        bad = true;
                        continue;
                    }
                };
                if let Some(v) = fortis_validate::validate(&prog).into_iter().find(|v| user_violation(v)) {
                    eprintln!("{f}: the typed IR is invalid ({} {}:{} {}): a compiler defect", v.check, v.function, v.line, v.message);
                    bad = true;
                    continue;
                }
                let base = pkg_dir.map(|d| d.to_path_buf()).unwrap_or_else(|| p.parent().unwrap_or(Path::new(".")).to_path_buf());
                // records name files as `fortis test` names them: relative, never absolute (D-302)
                let (report_file, decl_pairs) = report_files(f);
                let mut decl_files = std::collections::HashMap::<String, String>::new();
                for (name, path) in decl_pairs {
                    decl_files.entry(name).or_insert(path);
                }
                let opts = fortis_vm::testing::Options {
                    test: flag("--test"),
                    case: flag("--case"),
                    seed: flag("--seed").and_then(|s| s.parse().ok()),
                    cases: flag("--cases").and_then(|s| s.parse().ok()).unwrap_or(100),
                    fuzz: args.iter().any(|a| a == "--fuzz").then(|| fortis_vm::testing::Fuzz {
                        budget: flag("--budget").and_then(|s| s.parse().ok()).unwrap_or(10_000),
                        corpus: Some(flag("--corpus").map(std::path::PathBuf::from).unwrap_or_else(|| base.join(".fortis").join("fuzz"))),
                    }),
                    replay: flag("--replay").map(std::path::PathBuf::from),
                    command: format!("fortis test {f}"),
                    decl_files,
                    ..Default::default()
                };
                let out = fortis_vm::testing::run(&prog, &report_file, &opts);
                // a test's records are runtime records, on standard error; the summary alone is the command's
                // output, on standard output, as `fortis test` writes them (LR 4.6, 14)
                for r in &out.records {
                    if as_json {
                        eprintln!("{}", r.to_line());
                    } else {
                        eprintln!("{}", render_runtime(r));
                    }
                }
                let summary = fortis_diag::Json::obj([("kind", fortis_diag::Json::str("test-summary")), ("passed", fortis_diag::Json::Int(out.passed as i64)), ("failed", fortis_diag::Json::Int(out.failed as i64)), ("rule", fortis_diag::Json::str("PROG-TST-01"))]);
                if as_json {
                    outln!("{}", summary.to_line());
                } else {
                    outln!("{}: {} passed, {} failed", f, out.passed, out.failed);
                }
                bad |= out.failed > 0;
            }
            if bad {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Some("syntax") => {
            for f in files {
                let src = std::fs::read_to_string(f).unwrap_or_default();
                let o = fortis_syntax::check::judge(f, &src);
                match &o.code {
                    None => println!("{f}\tok\t{}", o.form),
                    Some(c) => println!("{f}\t{c}\t{}:{}\t{}", o.line, o.col, o.form),
                }
                for r in &o.records {
                    let errs = r.validate();
                    if !errs.is_empty() {
                        println!("{f}\tINVALID-RECORD\t{}", errs.join("; "));
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Some("summaries") => {
            for f in files {
                let src = read_module(f);
                println!("=== {f}");
                if let Ok(p) = fortis_syntax::parse_module(&src) {
                    let (_, text) = fortis_types::check::check_parsed_with(&p, f, |ty| fortis_types::effects::summaries_text(&ty.summaries));
                    print!("{text}");
                }
            }
            ExitCode::SUCCESS
        }
        Some("mir-library") => {
            let root = fortis_names::library::repo_root();
            let lib = fortis_mir::build::LibIndex::load(&root);
            for m in fortis_mir::build::LIBRARY_MODULES {
                match fortis_mir::build_library_module(&root, m, &lib) {
                    Ok(p) => println!("fortis.{m}: {} functions, {} types", p.functions.len(), p.types.len()),
                    Err((recs, errs)) => {
                        println!("fortis.{m}: not built");
                        for r in recs.iter().take(8) {
                            println!("  checker: {} {}:{} {}", r.code, r.line, r.column, r.message);
                        }
                        for e in errs.iter().take(8) {
                            println!("  unbuilt: line {}: {}", e.line, e.message);
                        }
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Some("validate") => {
            // the IR validator (DEL-45) over each program's typed IR, linked with the library
            let root = fortis_names::library::repo_root();
            let lib = fortis_mir::build::LibIndex::load(&root);
            let core = fortis_mir::library_program(&root, &lib);
            if args.iter().any(|a| a == "--coverage") {
                println!("{}", coverage(&root, &lib, &core).to_line());
                return ExitCode::SUCCESS;
            }
            // `--roundtrip`: each program's JSON form parses back to the same program
            // `--runs N`: the validator's report is identical over N runs (DEL-45 criterion 2)
            let runs: usize = args.iter().position(|a| a == "--runs").and_then(|i| args.get(i + 1)).and_then(|n| n.parse().ok()).unwrap_or(1);
            if args.iter().any(|a| a == "--roundtrip") || runs > 1 {
                let mut bad = 0;
                let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
                for f in &files {
                    let src = read_module(f);
                    let p = match build_target(f, &src, &lib) {
                        Ok(p) => fortis_mir::link(p, &core),
                        Err(_) => continue,
                    };
                    if args.iter().any(|a| a == "--roundtrip") {
                        let text = fortis_mir::json::program(&p).to_line();
                        match fortis_mir::json::parse_program(&text) {
                            Ok(q) if q == p => {}
                            Ok(_) => {
                                bad += 1;
                                println!("roundtrip differs: {f}");
                            }
                            Err(e) => {
                                bad += 1;
                                println!("roundtrip fails: {f}: {e}");
                            }
                        }
                    }
                    let mut first: Option<String> = None;
                    for _ in 0..runs {
                        let report = format!("{:?}", fortis_validate::validate(&p));
                        match &first {
                            None => first = Some(report),
                            Some(r) if *r != report => {
                                bad += 1;
                                println!("nondeterministic: {f}");
                            }
                            _ => {}
                        }
                    }
                    for b in first.unwrap_or_default().bytes() {
                        digest = (digest ^ b as u64).wrapping_mul(0x0100_0000_01b3);
                    }
                }
                println!("programs: {}; problems: {bad}; report digest: {digest:016x}", files.len());
                return if bad == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) };
            }
            let mut bad = false;
            for f in files {
                let src = read_module(f);
                println!("=== {f}");
                match build_target(f, &src, &lib) {
                    Ok(p) => {
                        let p = fortis_mir::link(p, &core);
                        let lib_only = args.iter().any(|a| a == "--library");
                        for v in fortis_validate::validate(&p).into_iter().filter(|v| lib_only || user_violation(v)) {
                            bad = true;
                            println!("violation: {} {}:{} {}", v.check, v.function, v.line, v.message);
                        }
                    }
                    Err((recs, errs)) => {
                        let first = recs.first().map(|r| format!("refused:{}", r.code)).or_else(|| errs.first().map(|e| format!("unbuilt:line {}: {}", e.line, e.message))).unwrap_or_default();
                        println!("skipped: {first}");
                    }
                }
            }
            if bad {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Some("native") => {
            // lower each program to a safe Rust crate (DEL-36), build it offline and run it
            let root = fortis_names::library::repo_root();
            let lib = fortis_mir::build::LibIndex::load(&root);
            let core = fortis_mir::library_program(&root, &lib);
            let out_dir = root.join("build").join("native-prod");
            let _ = std::fs::create_dir_all(&out_dir);
            let keep = args.iter().any(|a| a == "--emit-only");
            // a toolchain or backend failure ends non-zero; a program's own defined behaviour (a checker's
            // refusal of a negative, its failure, a defect, a dynamic error) is what the harnesses read on the
            // observed line and is not a failure of this command (D-55)
            let mut bad = false;
            for f in files {
                let src = read_module(f);
                println!("=== {f}");
                // `--tests` runs tests. A source comment does not (D-326): `// tests: run` is a harness marker, and
                // the harness passes the flag. A file under a package's src/ is that package (D-327). Under
                // `--tests`, that file still runs only its own module's tests (D-54).
                let tests = args.iter().any(|a| a == "--tests");
                let unit = run_unit(f);
                let module = if tests { unit.module.clone() } else { None };
                let check_at = unit.root.as_path();
                if let Some(r) = di_records(check_at).first().or(fortis_types::plans::check_path(check_at).first()) {
                    println!("observed: refused:{}", r.code);
                    continue;
                }
                let build_at = if check_at.is_dir() { check_at.to_string_lossy().into_owned() } else { f.to_string() };
                let built = if let Some(m) = &module {
                    build_package_module_target(&unit.root, m, &lib)
                } else {
                    build_target(&build_at, if check_at.is_dir() { "" } else { src.as_str() }, &lib)
                };
                let p = match built {
                    Ok(p) => fortis_mir::link(p, &core),
                    Err((recs, errs)) => {
                        let first = recs.first().map(|r| format!("refused:{}", r.code)).or_else(|| errs.first().map(|e| format!("unbuilt:line {}: {}", e.line, e.message))).unwrap_or_default();
                        println!("observed: {first}");
                        bad |= !first.starts_with("refused:");
                        continue;
                    }
                };
                let violations: Vec<fortis_validate::Violation> = fortis_validate::validate(&p).into_iter().filter(|v| user_violation(v)).collect();
                if let Some(v) = violations.first() {
                    println!("observed: refused-ir:{} {}:{} {}", v.check, v.function, v.line, v.message);
                    bad = true;
                    continue;
                }
                let stem = program_stem(std::path::Path::new(f));
                // the allocator probe build (FORTIS_ALLOC_PROBE=1, SP-04-async) is a separate crate
                let probe = std::env::var_os("FORTIS_ALLOC_PROBE").map_or(false, |v| v == "1");
                let crate_name = format!("fx_{}{}", stem.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect::<String>(), if probe { "_probe" } else { "" });
                let (report_file, decl_files) = report_files(f);
                let opts = fortis_lower::Options { runtime: root.join("compiler").join("crates").join("fortis-native").display().to_string(), crate_name: crate_name.clone(), file: report_file, decl_files, tests };
                let lowered = match fortis_lower::lower(&p, &opts) {
                    Ok(l) => l,
                    Err(es) => {
                        let e = &es[0];
                        println!("observed: unlowered:{}:{} {} ({} problems)", e.function, e.line, e.message, es.len());
                        bad = true;
                        continue;
                    }
                };
                let dir = out_dir.join(&crate_name);
                let _ = std::fs::create_dir_all(dir.join("src"));
                // unchanged files are not rewritten, so cargo's fingerprint finds nothing to rebuild
                write_if_changed(&dir.join("Cargo.toml"), &lowered.cargo_toml);
                write_if_changed(&dir.join("src").join("main.rs"), &lowered.main_rs);
                println!(
                    "audit: clones {} (unattributed 0), refcounts {}, boxed closures {}, boxed closure results {}",
                    lowered.audit.clones.len(),
                    lowered.audit.refcounts.len(),
                    lowered.audit.boxed_closures,
                    lowered.audit.boxed_results
                );
                if keep {
                    println!("observed: emitted {}", dir.display());
                    continue;
                }
                // progress redraws and color (`CARGO_TERM_PROGRESS_WHEN`, `CARGO_TERM_COLOR` in a parent environment)
                // are not part of the diagnostic; an explicit never wins over the parent's config, and the parser
                // below does not depend on either
                let build = std::process::Command::new("cargo").args(["build", "--color", "never", "--release", "--offline", "--quiet"]).current_dir(&dir).env("CARGO_TARGET_DIR", out_dir.join("target")).env("CARGO_TERM_PROGRESS_WHEN", "never").env("CARGO_TERM_COLOR", "never").output();
                match build {
                    Ok(o) if o.status.success() => {
                        // evidence states (Delivery Programme, evidence states): rustc's acceptance of the generated
                        // crate re-checks its loans and types; it is evidence about the generated code, never a
                        // proof about the source program, so it is recorded as `generated_checked`
                        let evidence = format!(
                            "{{\n  \"program\": {:?},\n  \"ir_validator\": \"validated\",\n  \"rustc\": \"generated_checked\",\n  \"unsafe_code\": \"forbidden\",\n  \"clones\": {},\n  \"unattributed_clones\": 0,\n  \"refcounts\": {},\n  \"boxed_closures\": {},\n  \"boxed_closure_results\": {}\n}}\n",
                            f,
                            lowered.audit.clones.len(),
                            lowered.audit.refcounts.len(),
                            lowered.audit.boxed_closures,
                            lowered.audit.boxed_results
                        );
                        let _ = std::fs::write(dir.join("evidence.json"), evidence);
                        println!("evidence: ir_validator=validated rustc=generated_checked unsafe_code=forbidden");
                    }
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr);
                        let first = build::compiler_error_lines(&err).into_iter().take(2).collect::<Vec<_>>().join(" / ");
                        println!("observed: build-error:{first}");
                        bad = true;
                        continue;
                    }
                    Err(e) => {
                        println!("observed: build-error:{e}");
                        bad = true;
                        continue;
                    }
                }
                let exe = out_dir.join("target").join("release").join(&crate_name);
                // the executor trace is reported as a record, for the fixtures that fix one (`execute-trace`)
                let mut cmd = std::process::Command::new(&exe);
                if !probe {
                    cmd.env("FORTIS_TRACE", "1");
                }
                let run = cmd.output();
                match run {
                    Ok(o) => {
                        let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                        let mut lines: Vec<String> = stdout.lines().map(|l| l.to_string()).collect();
                        let last_failure = stderr.lines().filter(|l| l.contains("\"kind\": \"runtime-failure\"")).last().and_then(|l| l.split("\"code\": \"").nth(1)).and_then(|r| r.split('"').next()).map(|c| c.to_string());
                        let defect = stderr.lines().filter(|l| l.contains("\"kind\": \"defect\"")).last().and_then(|l| l.split("\"class\": \"").nth(1)).and_then(|r| r.split('"').next()).map(|c| c.to_string());
                        if o.status.code() == Some(2) {
                            if let Some(d) = stderr.lines().find(|l| l.starts_with("dynamic:")) {
                                println!("observed: {d}");
                                continue;
                            }
                        }
                        match o.status.code() {
                            Some(0) => {}
                            // a test run that failed or left a test unbuilt: the runner's own non-zero exit (D-55)
                            Some(1) if tests => bad = true,
                            Some(1) => lines.push(format!("fail:{}", last_failure.unwrap_or_default())),
                            Some(101) => lines.push(format!("defect:{}", defect.unwrap_or_default())),
                            // the platform's guard page aborting a recursion that ran out of stack (10.8: native abort)
                            None if stderr.contains("has overflowed its stack") => lines.push("defect:stack_exhaustion".to_string()),
                            other => {
                                lines.push(format!("exit:{other:?}"));
                                bad = true;
                            }
                        }
                        println!("observed: {}", lines.join("|"));
                        for l in stderr.lines() {
                            println!("record: {l}");
                        }
                    }
                    Err(e) => {
                        println!("observed: run-error:{e}");
                        bad = true;
                    }
                }
            }
            if bad {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Some("dev") => {
            // a development session (Platform 11; spike SP-07): `fortisc dev --script S.json [--events F] [--metrics F]`
            let script = match flag("--script") {
                Some(s) => s,
                None if args.iter().any(|a| a == "--ir-size") => String::new(),
                None => {
                    eprintln!("usage: fortisc dev --script SCRIPT.json [--events FILE] [--metrics FILE]");
                    return ExitCode::from(2);
                }
            };
            let root = fortis_names::library::repo_root();
            let lib = fortis_mir::build::LibIndex::load(&root);
            let core = fortis_mir::library_program(&root, &lib);
            // the same pipeline as `fortisc vm`: the DI kit, plan compatibility, the checker and the IR, linking,
            // and the validator
            let compile_source = |path: &Path| -> Result<fortis_vm::session::Compiled, Vec<String>> {
                let f = path.to_string_lossy().to_string();
                let raw = std::fs::read(path).map_err(|e| vec![format!("E-INT-002 {f}: {e}")])?;
                let src = read_module(&f);
                let kit_records: Vec<Record> = di_records(path).into_iter().chain(fortis_types::plans::check_path(path)).collect();
                if !kit_records.is_empty() {
                    return Err(kit_records.iter().map(|r| format!("{} {}:{} {}", r.code, r.line, r.column, r.message)).collect());
                }
                let p = match build_target(&f, &src, &lib) {
                    Ok(p) => fortis_mir::link(p, &core),
                    Err((recs, errs)) => return Err(recs.iter().map(|r| format!("{} {}:{} {}", r.code, r.line, r.column, r.message)).chain(errs.iter().map(|e| format!("E-INT-001 {}: {}", e.line, e.message))).collect()),
                };
                let violations: Vec<String> = fortis_validate::validate(&p).into_iter().filter(|v| user_violation(v)).map(|v| format!("refused-ir {} {}:{} {}", v.check, v.function, v.line, v.message)).collect();
                if !violations.is_empty() {
                    return Err(violations);
                }
                let di = fortis_di::plans(path)
                    .iter()
                    .filter_map(|j| {
                        let ctx = j.get("context")?.as_str()?.to_string();
                        let mut classes = vec![ctx.clone()];
                        for l in j.get("levels").and_then(|l| l.as_arr()).map(|v| v.as_slice()).unwrap_or(&[]) {
                            if let Some(c) = l.get("class").and_then(|c| c.as_str()) {
                                classes.push(c.to_string());
                            }
                        }
                        Some((ctx, classes))
                    })
                    .collect();
                Ok(fortis_vm::session::Compiled { program: p, sha256: fortis_names::sha256::sha256(&raw), di, generic: declared_generic(&src) })
            };
            let compile = |path: &Path| -> Result<fortis_vm::session::Compiled, Vec<String>> {
                let r = compile_source(path);
                // the session compiles many sources in one process: the DI registry keeps nothing of this one
                fortis_di::forget(path);
                r
            };
            let mut sink: Box<dyn std::io::Write + Send> = match flag("--events") {
                Some(f) => match std::fs::File::create(&f) {
                    Ok(file) => Box::new(std::io::BufWriter::new(file)),
                    Err(e) => {
                        eprintln!("fortisc dev: {f}: {e}");
                        return ExitCode::from(2);
                    }
                },
                None => Box::new(std::io::stdout()),
            };
            let mut frames: Option<Box<dyn std::io::Write + Send>> = match flag("--frames") {
                Some(f) => match std::fs::File::create(&f) {
                    Ok(file) => Some(Box::new(std::io::BufWriter::new(file))),
                    Err(e) => {
                        eprintln!("fortisc dev: {f}: {e}");
                        return ExitCode::from(2);
                    }
                },
                None => None,
            };
            if args.iter().any(|a| a == "--ir-size") {
                // the size the memory cap counts, measured apart from the session (SP-07's G3 checks the session's)
                for f in &files {
                    match compile(Path::new(f.as_str())) {
                        Ok(c) => {
                            let mut out = String::new();
                            fortis_mir::json::program(&c.program).write(&mut out);
                            println!("{} {f}", out.len());
                        }
                        Err(d) => println!("rejected {f}: {}", d.join("; ")),
                    }
                }
                return ExitCode::SUCCESS;
            }
            match fortis_vm::session::run_script(&compile, Path::new(&script), &mut *sink, frames.as_deref_mut().map(|w| w as &mut (dyn std::io::Write + Send))) {
                Ok(metrics) => {
                    if let Some(f) = flag("--metrics") {
                        let _ = std::fs::write(f, metrics.iter().map(|e| format!("{e}\n")).collect::<String>());
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("fortisc dev: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Some("vm") => {
            // run each program's `main` on the VM; print what the conformance harness compares.
            // The process exit is the program's (LR 4.6): 0 success, 1 a failing main or a refusal, 2 a dynamic
            // check, 101 a defect. Several files take the worst of those. A source comment does not select tests
            // (D-326). A file under a package's src/ runs that package's program (D-327).
            let root = fortis_names::library::repo_root();
            let lib = fortis_mir::build::LibIndex::load(&root);
            let core = fortis_mir::library_program(&root, &lib);
            let tests = args.iter().any(|a| a == "--tests");
            let mut worst = 0;
            for f in files {
                let src = read_module(f);
                println!("=== {f}");
                let unit = run_unit(f);
                let check_at = unit.root.as_path();
                // the DI kit's findings, then plan compatibility, the build-time step check does not run (PROG-VM-04)
                if let Some(r) = di_records(check_at).first().or(fortis_types::plans::check_path(check_at).first()) {
                    println!("observed: refused:{}", r.code);
                    worst = worse_exit(worst, 1);
                    continue;
                }
                let build_at = if check_at.is_dir() { check_at.to_string_lossy().into_owned() } else { f.to_string() };
                let built = if tests {
                    if let Some(m) = &unit.module {
                        build_package_module_target(&unit.root, m, &lib)
                    } else {
                        build_target(&build_at, if check_at.is_dir() { "" } else { src.as_str() }, &lib)
                    }
                } else {
                    build_target(&build_at, if check_at.is_dir() { "" } else { src.as_str() }, &lib)
                };
                match built {
                    Ok(p) => {
                        let mut p = fortis_mir::link(p, &core);
                        // `--corrupt ID`: apply the validator's corruption for a check first (DEL-35 criterion 2)
                        if let Some(i) = args.iter().position(|a| a == "--corrupt") {
                            let id = args.get(i + 1).cloned().unwrap_or_default();
                            fortis_validate::corrupt::corrupt(&id, &mut p);
                        }
                        // the VM consumes only validated IR (DEL-45): a program the validator rejects is refused
                        let violations: Vec<fortis_validate::Violation> = fortis_validate::validate(&p).into_iter().filter(|v| user_violation(v)).collect();
                        if let Some(v) = violations.first() {
                            println!("observed: refused-ir:{} {}:{} {}", v.check, v.function, v.line, v.message);
                            worst = worse_exit(worst, 1);
                            continue;
                        }
                        // `--probe`: the peak number of OS threads while the program runs (PROG-NFR-14: no executor
                        // thread without a launch); a watcher samples /proc/self/task
                        let probe = args.iter().any(|a| a == "--probe");
                        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                        let threads = || std::fs::read_dir("/proc/self/task").map(|d| d.count()).unwrap_or(0);
                        // the VM's runner thread stands in for this one, which waits for it
                        let baseline = threads() + fortis_vm::RUNNER_THREADS;
                        let watcher = if probe {
                            let (stop, peak) = (stop.clone(), peak.clone());
                            Some(std::thread::spawn(move || {
                                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                                    let n = std::fs::read_dir("/proc/self/task").map(|d| d.count()).unwrap_or(0).saturating_sub(1);
                                    peak.fetch_max(n, std::sync::atomic::Ordering::Relaxed);
                                    std::thread::sleep(std::time::Duration::from_micros(200));
                                }
                            }))
                        } else {
                            None
                        };
                        let (report_file, decl_pairs) = report_files(f);
                        let mut decl_files = std::collections::HashMap::<String, String>::new();
                        for (name, path) in decl_pairs {
                            decl_files.entry(name).or_insert(path);
                        }
                        let o = if tests { fortis_vm::run_tests(&p, &report_file, &decl_files) } else { fortis_vm::run_main(&p, &report_file, &[], &decl_files) };
                        if let Some(w) = watcher {
                            stop.store(true, std::sync::atomic::Ordering::Relaxed);
                            let _ = w.join();
                            let tasks = o.trace.iter().filter(|e| e.starts_with("launch ")).count();
                            println!("probe: baseline {baseline} peak {} launches {tasks} threads {}", peak.load(std::sync::atomic::Ordering::Relaxed).max(baseline), o.threads);
                        }
                        println!("observed: {}", o.text);
                        println!("trace: {}", o.trace.join("|"));
                        for r in &o.reports {
                            println!("record: {}", r.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "));
                        }
                        // one failure record on stderr, the same JSON the interpreter writes (LR 4.6)
                        for line in &o.failure_lines {
                            eprintln!("{line}");
                        }
                        worst = worse_exit(worst, o.exit_code);
                    }
                    Err((recs, errs)) => {
                        let first = recs.first().map(|r| format!("refused:{}", r.code)).or_else(|| errs.first().map(|e| format!("unbuilt:line {}: {}", e.line, e.message))).unwrap_or_default();
                        println!("observed: {first}");
                        worst = worse_exit(worst, 1);
                    }
                }
            }
            ExitCode::from(worst as u8)
        }
        Some("mir") => {
            let root = fortis_names::library::repo_root();
            let lib = fortis_mir::build::LibIndex::load(&root);
            let mut bad = false;
            for f in files {
                let src = read_module(f);
                match build_target(f, &src, &lib) {
                    Ok(p) => {
                        if json {
                            println!("{}", fortis_mir::json::program(&p).to_line());
                        } else {
                            println!("=== {f}");
                            print!("{}", fortis_mir::print::program(&p));
                        }
                    }
                    Err((recs, errs)) => {
                        bad = true;
                        println!("=== {f}");
                        for r in recs {
                            println!("checker: {} {}:{} {}", r.code, r.line, r.column, r.message);
                        }
                        for e in errs {
                            println!("unbuilt: line {}: {}", e.line, e.message);
                        }
                    }
                }
            }
            if bad {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Some("ir") => {
            for f in files {
                let p = std::path::Path::new(f);
                if p.is_dir() {
                    let search = p.parent().unwrap_or(p);
                    for (file, objs) in fortis_ir::function::package_ir(p, search).unwrap_or_default() {
                        println!("=== {file}");
                        for j in objs {
                            println!("{}", j.to_line());
                        }
                    }
                    continue;
                }
                let src = read_module(f);
                println!("=== {f}");
                if let Ok(p) = fortis_syntax::parse_module(&src) {
                    for j in fortis_ir::function::function_ir(&p, f) {
                        println!("{}", j.to_line());
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Some("decls") => {
            for f in files {
                let src = read_module(f);
                println!("=== {f}");
                if let Ok(p) = fortis_syntax::parse_module(&src) {
                    print!("{}", fortis_ir::decl_ir(&p, f));
                }
            }
            ExitCode::SUCCESS
        }
        Some("cfg") => {
            for f in files {
                let src = read_module(f);
                if let Ok(p) = fortis_syntax::parse_module(&src) {
                    for (name, text) in fortis_ir::cfgs(&p) {
                        println!("=== {f} {name}");
                        print!("{text}");
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Some("parse") => {
            for f in files {
                let src = std::fs::read_to_string(f).unwrap_or_default();
                println!("=== {f}");
                match fortis_syntax::parse_module(&src) {
                    Ok(p) => {
                        let mut out = String::new();
                        fortis_syntax::tree::dump(&p.root, &p.toks, &mut out, 0);
                        print!("{out}");
                    }
                    Err(fortis_syntax::SyntaxError::Lex(e)) => println!("LEXERROR {} {}:{}", e.code, e.line, e.col),
                    Err(fortis_syntax::SyntaxError::Parse(e)) => println!("PARSEERROR {}:{}", e.line, e.col),
                    Err(fortis_syntax::SyntaxError::Limit(e)) => println!("LIMIT E-LIM-002 {}:{} {e}", e.line, e.col),
                }
            }
            ExitCode::SUCCESS
        }
        // `fmt [--check|--write] PATH...`: the canonical layout (DEL-41). Without a flag the formatted text goes to
        // standard output; `--write` rewrites the files that change; `--check` lists them and exits 1 if any would.
        // A file that does not parse is not formatted (exit 1); a result that would not be the same program is a
        // formatter defect and nothing is written (exit 4).
        Some("elaborate") => {
            // the module as the core reads it (the DI kit's elaboration, SP-08); `--plan`: the resolved plans
            let plan = args.iter().any(|a| a == "--plan");
            let mut bad = false;
            for f in files {
                let p = Path::new(f);
                let text = read_module(p);
                let recs = di_records(p);
                if plan {
                    for j in fortis_di::plans(p) {
                        outln!("{}", j.to_line());
                    }
                } else {
                    outln!("{text}");
                }
                for r in &recs {
                    eprintln!("{}", r.to_json_line());
                }
                // `--origins`: the source line of each elaborated line, for a reader that checks the elaborated text
                if args.iter().any(|a| a == "--origins") {
                    eprintln!("{}", fortis_diag::Json::obj([("origins", fortis_diag::Json::Arr(fortis_di::origins(p).into_iter().map(|o| fortis_diag::Json::Int(o as i64)).collect()))]).to_line());
                }
                bad |= !recs.is_empty();
            }
            if bad {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Some("fmt") => {
            let write = args.iter().any(|a| a == "--write");
            let check = args.iter().any(|a| a == "--check");
            let mut paths: Vec<std::path::PathBuf> = Vec::new();
            for f in &files {
                let p = std::path::Path::new(f.as_str());
                if p.is_file() {
                    paths.push(p.to_path_buf());
                } else {
                    ft_files(p, &mut paths);
                }
            }
            let (mut changed, mut failed, mut defect) = (0, 0, 0);
            for p in &paths {
                let src = match std::fs::read_to_string(p) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{}: {e}", p.display());
                        failed += 1;
                        continue;
                    }
                };
                // `--layout`: whitespace only, without printing back inferred facts
                // the v0.0.2 reading migrations of the entry `main`, a test's `or fail` and a map lookup come first
                // (`fortis_fmt::migrate`, X-345), unless `--no-migrate` or `--layout` asks for none: an entry module's
                // `main` (a standalone file, or the manifest's `entry`) is the entry `main`
                let layout = args.iter().any(|a| a == "--layout");
                let migrated = if layout || args.iter().any(|a| a == "--no-migrate") {
                    src.clone()
                } else {
                    let file = p.display().to_string();
                    match enclosing_package(p) {
                        None => fortis_fmt::migrate::migrate(&src, true, &fortis_fmt::migrate::facts_alone(&file)),
                        Some((dir, module)) => {
                            let entry = fortis_names::project::Package::load(&dir).ok().and_then(|pk| pk.entry).map_or(false, |e| e == module);
                            fortis_fmt::migrate::migrate(&src, entry, &fortis_fmt::migrate::facts_in_package(&dir, p))
                        }
                    }
                };
                // the rewrites into the v0.0.2 reading forms are counted on standard error (X-345); a module of a
                // package is checked in its package, so that its imports resolve
                let formatted = if layout {
                    fortis_fmt::format(&src)
                } else {
                    let file = p.display().to_string();
                    let package = enclosing_package(p);
                    let unit = match &package {
                        None => fortis_fmt::Unit::Alone(&file),
                        Some((dir, _)) => fortis_fmt::Unit::Package { dir, file: p },
                    };
                    fortis_fmt::format_migrating_in(&migrated, unit).map(|(t, _, n)| {
                        if n > 0 {
                            eprintln!("{}: {n} rewrite{} into the reading forms", p.display(), if n == 1 { "" } else { "s" });
                        }
                        t
                    })
                };
                match formatted {
                    Ok(out) => {
                        let differs = out != src;
                        if check {
                            if differs {
                                println!("{}", p.display());
                                changed += 1;
                            }
                        } else if write {
                            if differs {
                                if let Err(e) = std::fs::write(p, &out) {
                                    eprintln!("{}: {e}", p.display());
                                    failed += 1;
                                } else {
                                    changed += 1;
                                }
                            }
                        } else {
                            if paths.len() > 1 {
                                println!("=== {}", p.display());
                            }
                            print!("{out}");
                        }
                    }
                    Err(e @ fortis_fmt::FormatError::Syntax(_)) => {
                        eprintln!("{}: {e}", p.display());
                        failed += 1;
                    }
                    Err(e) => {
                        eprintln!("{}: {e}", p.display());
                        defect += 1;
                    }
                }
            }
            if write {
                eprintln!("{changed} of {} files reformatted", paths.len());
            }
            if defect > 0 {
                ExitCode::from(4)
            } else if failed > 0 || (check && changed > 0) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        // `lock [--check] [--search DIR] PACKAGE_DIR` (DEL-42): write the package's fortis.lock from its manifests,
        // or verify it (exit 1 with a record for each divergence).
        Some("lock") => {
            let check = args.iter().any(|a| a == "--check");
            let toolchain = !args.iter().any(|a| a == "--no-toolchain");
            let search_arg = args.iter().position(|a| a == "--search").and_then(|i| args.get(i + 1)).cloned();
            let mut bad = false;
            for f in files.iter().filter(|f| Some(f.as_str()) != search_arg.as_deref()) {
                let dir = Path::new(f.as_str());
                let search = search_arg.as_deref().map(Path::new).unwrap_or_else(|| dir.parent().unwrap_or(Path::new(".")));
                if check {
                    let text = match std::fs::read_to_string(dir.join(fortis_names::lock::FILE)) {
                        Ok(t) => t,
                        Err(_) => {
                            eprintln!("{}: no {}", dir.display(), fortis_names::lock::FILE);
                            bad = true;
                            continue;
                        }
                    };
                    let records = match fortis_names::lock::Lock::parse(&text) {
                        Ok(l) => l.verify(dir, search, if toolchain { fortis_names::lock::Pins::All } else { fortis_names::lock::Pins::Packages }),
                        Err(e) => {
                            eprintln!("{}: {e}", dir.display());
                            bad = true;
                            continue;
                        }
                    };
                    for r in &records {
                        println!("{}", if json { r.to_json_line() } else { format!("{}:{}:{}: error {} [{}]: {}", r.file, r.line, r.column, r.code, r.rule, r.message) });
                    }
                    if records.is_empty() {
                        eprintln!("{}: the lockfile holds", dir.display());
                    }
                    bad |= !records.is_empty();
                } else {
                    match fortis_names::lock::Lock::compute(dir, search) {
                        Ok((lock, records)) => {
                            for r in &records {
                                println!("{}", if json { r.to_json_line() } else { format!("{}:{}:{}: error {} [{}]: {}", r.file, r.line, r.column, r.code, r.rule, r.message) });
                            }
                            if !records.is_empty() {
                                bad = true;
                                continue;
                            }
                            let path = dir.join(fortis_names::lock::FILE);
                            if let Err(e) = std::fs::write(&path, lock.render()) {
                                eprintln!("{}: {e}", path.display());
                                bad = true;
                            } else {
                                eprintln!("{}: {} packages pinned", path.display(), lock.packages.len());
                            }
                        }
                        Err(e) => {
                            eprintln!("{}: {e}", dir.display());
                            bad = true;
                        }
                    }
                }
            }
            if bad {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        // `view [--json|--plain] [--decls | --symbol NAME | --query Q | --queries] PATH|GLOB...` (DEL-41): the reader
        // for people and agents. Highlighted on a terminal, plain text or JSON lines otherwise; never writes.
        Some("view") => {
            use std::io::IsTerminal;
            let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
            if args.iter().any(|a| a == "--queries") {
                for (q, d) in fortis_view::QUERIES {
                    if json {
                        outln!("{{\"description\": {}, \"kind\": \"query\", \"name\": {}}}", fortis_view::js(d), fortis_view::js(q));
                    } else {
                        outln!("{q:<16} {d}");
                    }
                }
                return ExitCode::SUCCESS;
            }
            let tty = std::io::stdout().is_terminal() && !json && !args.iter().any(|a| a == "--plain");
            let (query, symbol, decls) = (value("--query"), value("--symbol"), args.iter().any(|a| a == "--decls"));
            let mut paths: Vec<std::path::PathBuf> = Vec::new();
            for f in &files {
                view_paths(f, &mut paths);
            }
            if paths.is_empty() {
                eprintln!("fortisc view: no .ft files match");
                return ExitCode::from(2);
            }
            let mut unparsed = 0;
            for p in &paths {
                let shown = p.display().to_string();
                let Ok(text) = std::fs::read_to_string(p) else {
                    eprintln!("{shown}: unreadable");
                    unparsed += 1;
                    continue;
                };
                if query.is_some() || symbol.is_some() || decls {
                    let Some(src) = fortis_view::parse(&text) else {
                        eprintln!("{shown}: does not parse; `fortisc check` reports why");
                        unparsed += 1;
                        continue;
                    };
                    if let Some(q) = &query {
                        match fortis_view::query(q, &src) {
                            Ok(ms) => {
                                for m in ms {
                                    if json {
                                        outln!("{{\"column\": {}, \"kind\": \"match\", \"line\": {}, \"path\": {}, \"query\": {}, \"text\": {}}}", m.col, m.line, fortis_view::js(&shown), fortis_view::js(q), fortis_view::js(&m.text));
                                    } else {
                                        outln!("{shown}:{}:{}: {}", m.line, m.col, m.text);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("{e}");
                                return ExitCode::from(2);
                            }
                        }
                        continue;
                    }
                    let ds = fortis_view::declarations(&text, &src);
                    let lines: Vec<&str> = text.split('\n').collect();
                    for d in ds {
                        let qualified = match &d.member_of {
                            Some(o) => format!("{o}.{}", d.name),
                            None => d.name.clone(),
                        };
                        if let Some(sym) = &symbol {
                            if *sym != qualified && *sym != d.name {
                                continue;
                            }
                            let body: Vec<&str> = lines.iter().skip(d.start_line - 1).take(d.end_line + 1 - d.start_line).copied().collect();
                            if json {
                                outln!("{{\"end_line\": {}, \"kind\": \"symbol\", \"line\": {}, \"name\": {}, \"path\": {}, \"text\": {}}}", d.end_line, d.start_line, fortis_view::js(&qualified), fortis_view::js(&shown), fortis_view::js(&body.join("\n")));
                            } else {
                                outln!("{shown}:{}:", d.start_line);
                                for (k, l) in body.iter().enumerate() {
                                    let line = fortis_view::Line { number: d.start_line + k, text: l.to_string(), spans: fortis_view::lines(&text).get(d.start_line + k - 1).map(|x| x.spans.clone()).unwrap_or_default() };
                                    outln!("{}", if tty { fortis_view::highlight(&line) } else { line.text.clone() });
                                }
                            }
                            continue;
                        }
                        if json {
                            let list = |v: &[String]| format!("[{}]", v.iter().map(|x| fortis_view::js(x)).collect::<Vec<_>>().join(", "));
                            outln!(
                                "{{\"aspects\": {}, \"column\": {}, \"contracts\": {}, \"declaration\": {}, \"doc\": {}, \"exported\": {}, \"head\": {}, \"kind\": \"declaration\", \"line\": {}, \"member_of\": {}, \"modifiers\": {}, \"name\": {}, \"path\": {}}}",
                                list(&d.aspects), d.col, list(&d.contracts), fortis_view::js(&d.kind), fortis_view::js(&d.doc), d.exported, fortis_view::js(&d.head), d.line,
                                d.member_of.as_deref().map_or("null".to_string(), fortis_view::js), list(&d.modifiers), fortis_view::js(&d.name), fortis_view::js(&shown)
                            );
                        } else {
                            let indent = if d.member_of.is_some() { "  " } else { "" };
                            let aspects = if d.aspects.is_empty() { String::new() } else { format!("{} ", d.aspects.join(" ")) };
                            outln!("{shown}:{}:{}: {indent}{aspects}{}{} {}  — {}", d.line, d.col, if d.exported { "export " } else { "" }, d.kind, qualified, d.head);
                            if !d.doc.is_empty() {
                                outln!("    {indent}// {}", d.doc.replace('\n', " "));
                            }
                        }
                    }
                    continue;
                }
                if paths.len() > 1 && !json {
                    outln!("{}", if tty { format!("\u{1b}[1m=== {shown}\u{1b}[0m") } else { format!("=== {shown}") });
                }
                for l in fortis_view::lines(&text) {
                    if json {
                        let spans: Vec<String> = l.spans.iter().map(|(c, n, k)| format!("[{c}, {n}, {}]", fortis_view::js(k))).collect();
                        outln!("{{\"kind\": \"line\", \"line\": {}, \"path\": {}, \"spans\": [{}], \"text\": {}}}", l.number, fortis_view::js(&shown), spans.join(", "), fortis_view::js(&l.text));
                    } else if tty {
                        outln!("{}", fortis_view::highlight(&l));
                    } else {
                        outln!("{}", l.text);
                    }
                }
            }
            if unparsed > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        _ => {
            eprintln!("usage: fortisc check|syntax|parse|fmt|view FILE...");
            ExitCode::from(2)
        }
    }
}

/// The files a `view` argument names: a file, the `.ft` files below a directory, or the `.ft` files a glob
/// (`*`, `**`, `?`) matches, walked from its literal prefix.
fn view_paths(arg: &str, out: &mut Vec<std::path::PathBuf>) {
    if !arg.contains('*') && !arg.contains('?') {
        let p = std::path::Path::new(arg);
        if p.is_file() {
            out.push(p.to_path_buf());
        } else {
            ft_files(p, out);
        }
        return;
    }
    let parts: Vec<&str> = arg.split('/').collect();
    let lit = parts.iter().take_while(|s| !s.contains('*') && !s.contains('?')).cloned().collect::<Vec<_>>().join("/");
    let base = if lit.is_empty() { ".".to_string() } else { lit };
    let mut all = Vec::new();
    ft_files(std::path::Path::new(&base), &mut all);
    for p in all {
        let s = p.display().to_string();
        let rel = s.strip_prefix("./").unwrap_or(&s);
        if fortis_view::glob_match(arg.strip_prefix("./").unwrap_or(arg), rel) {
            out.push(p);
        }
    }
}

/// The `.ft` files of a path: the file itself, or every `.ft` file below a directory (sorted, hidden directories
/// and build output skipped).
fn ft_files(p: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if p.is_dir() {
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(p).map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
        entries.sort();
        for e in entries {
            let name = e.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "target" || name == "build" {
                continue;
            }
            ft_files(&e, out);
        }
    } else if p.extension().map_or(false, |x| x == "ft") {
        out.push(p.to_path_buf());
    }
}

/// The validator's coverage report (DEL-45 criteria 1 and 3): for every check of the registry, its G1 fixture
/// (compiler/crates/fortis-validate/fixtures/<id>.ft) must validate and its targeted corruption must be rejected.
fn coverage(root: &std::path::Path, lib: &fortis_mir::build::LibIndex, core: &fortis_mir::Program) -> fortis_diag::Json {
    use fortis_diag::Json;
    let dir = root.join("compiler").join("crates").join("fortis-validate").join("fixtures");
    let mut rows = Vec::new();
    for c in fortis_validate::checks::CHECKS {
        let mut row = vec![("id", Json::str(c.id)), ("row", Json::str(c.row)), ("property", Json::str(c.property)), ("subset", Json::str(c.subset))];
        let fixture = dir.join(format!("{}.ft", c.id));
        if c.id.starts_with("wf.") {
            row.push(("covered", Json::Bool(true)));
            row.push(("note", Json::str("an IR invariant: exercised by every program")));
            rows.push(Json::obj(row));
            continue;
        }
        let src = match std::fs::read_to_string(&fixture) {
            Ok(s) => s,
            Err(_) => {
                row.push(("covered", Json::Bool(false)));
                row.push(("note", Json::str("no fixture")));
                rows.push(Json::obj(row));
                continue;
            }
        };
        row.push(("fixture", Json::str(format!("compiler/crates/fortis-validate/fixtures/{}.ft", c.id))));
        let built = fortis_mir::build_source(&src, &fixture.display().to_string(), lib);
        let prog = match built {
            Ok(p) => fortis_mir::link(p, core),
            Err((recs, errs)) => {
                let why = recs.first().map(|r| format!("checker {} at {}: {}", r.code, r.line, r.message)).or_else(|| errs.first().map(|e| format!("unbuilt at {}: {}", e.line, e.message))).unwrap_or_default();
                row.push(("covered", Json::Bool(false)));
                row.push(("note", Json::str(why)));
                rows.push(Json::obj(row));
                continue;
            }
        };
        let user = |vs: Vec<fortis_validate::Violation>| -> Vec<fortis_validate::Violation> { vs.into_iter().filter(|v| user_violation(v)).collect() };
        let valid = user(fortis_validate::validate(&prog));
        let mut corrupted = prog.clone();
        let applied = fortis_validate::corrupt::corrupt(c.id, &mut corrupted);
        let after = user(fortis_validate::validate(&corrupted));
        let new: Vec<&fortis_validate::Violation> = after.iter().filter(|v| !valid.contains(v)).collect();
        let by: Vec<String> = {
            let mut b: Vec<String> = new.iter().map(|v| v.check.to_string()).collect();
            b.sort();
            b.dedup();
            b
        };
        row.push(("valid_violations", Json::Arr(valid.iter().map(|v| Json::str(format!("{} {}:{} {}", v.check, v.function, v.line, v.message))).collect())));
        row.push(("corruption_applied", Json::Bool(applied)));
        row.push(("rejected_by", Json::strs(by.clone())));
        row.push(("rejection", Json::Arr(new.iter().map(|v| Json::str(format!("{} {}:{} {}", v.check, v.function, v.line, v.message))).collect())));
        // the row's own check must reject its corruption (another check catching it is not coverage of this row); a
        // whitelist pattern is a permission, so its corruption (an operation off the list) is caught by a C.3 check
        let whitelist = c.property == "whitelisted";
        let caught = by.iter().any(|b| b == c.id) || (whitelist && by.iter().any(|b| b.starts_with("C3.")));
        row.push(("covered", Json::Bool(valid.is_empty() && applied && caught)));
        rows.push(Json::obj(row));
    }
    Json::obj([("checks", Json::Arr(rows))])
}
