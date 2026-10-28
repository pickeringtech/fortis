//! The typed IR of the Fortis compiler (Programme 5.2; PROG-ARC-01): the single program form that the validator
//! (DEL-45) checks and that the VM (DEL-35) and the native lowering (DEL-36) consume.

pub mod build;
pub mod json;
pub mod model;
pub mod names;
pub mod print;

pub use model::*;

use fortis_diag::Record;

/// Check one module and build its typed IR. The checker's records come first: a module with findings is not built.
/// A package directory (project.yaml and src/): every module checked and built into one program whose entry is the
/// entry module's `main` and whose tests are every module's. Dependencies other than `fortis` are built too.
pub fn build_package(dir: &std::path::Path, search: &std::path::Path, lib: &build::LibIndex) -> Result<Program, (Vec<Record>, Vec<build::BuildError>)> {
    build_package_with(dir, search, lib, None)
}

/// The package's program holding only the tests of one module of the root package: `fortis test FILE.ft` on a
/// module of a package (D-54) builds the whole package, so the module's imports resolve exactly as checking the
/// package resolves them, and runs that module's tests and no other module's.
pub fn build_package_module(dir: &std::path::Path, search: &std::path::Path, lib: &build::LibIndex, module: &str) -> Result<Program, (Vec<Record>, Vec<build::BuildError>)> {
    build_package_with(dir, search, lib, Some(module))
}

fn build_package_with(dir: &std::path::Path, search: &std::path::Path, lib: &build::LibIndex, tests_of: Option<&str>) -> Result<Program, (Vec<Record>, Vec<build::BuildError>)> {
    let mut ps = match fortis_names::resolve::PackageSet::load(dir, search) {
        Ok(ps) => ps,
        Err(e) => return Err((vec![fortis_names::resolve::manifest_record(&e, &dir.join("project.yaml").display().to_string())], vec![])),
    };
    let resolved = ps.run();
    if resolved.iter().any(|r| r.severity == "error") {
        return Err((resolved, vec![]));
    }
    // two versions of one package, or one type name declared by two modules: the checker keeps them apart by type
    // identity (LANG-04-04), and the typed IR names them apart (X-136)
    let mut lib = lib.clone();
    let declared: Vec<(String, std::collections::BTreeSet<String>)> =
        ps.packages.iter().flat_map(|(_, q)| q.modules.iter().map(move |(mpath, m)| (format!("{}/{}.{}", q.identity(), q.name, mpath), declared_types(m)))).collect();
    lib.qualify = names::Qualify::of(declared.iter().map(|(i, t)| (i.as_str(), t.clone())));
    let lib = &lib;
    let root_key = ps.root_key.clone();
    let root_identity = ps.root().identity();
    let (records, built) = fortis_types::check::check_package_set_typed(&ps, |ty, root, key, pkg| {
        if pkg.name == "fortis" {
            return None;
        }
        Some((build::build_module(ty, root, key, lib), pkg.name.clone(), pkg.entry.clone(), pkg.identity() == root_identity))
    });
    let errors: Vec<Record> = records.into_iter().filter(|r| r.severity == "error").collect();
    if !errors.is_empty() {
        return Err((errors, vec![]));
    }
    let mut out = Program::default();
    let mut build_errors = Vec::new();
    let mut entry = None;
    for (key, r) in built {
        let (p, pkg, pkg_entry, is_root) = match r {
            Some(x) => x,
            None => continue,
        };
        match p {
            Ok(p) => {
                // the root package's entry module supplies `main`
                let is_entry_module = pkg_entry.as_deref().map_or(false, |e| key == format!("{pkg}.{e}")) && (ps.root_key == pkg || root_key.starts_with(&format!("{pkg}@")));
                if is_entry_module {
                    entry = p.entry.clone();
                }
                out.functions.extend(p.functions);
                out.types.extend(p.types);
                out.consts.extend(p.consts);
                // with `tests_of`, only that module of the root package keeps its tests (a dependency's module of
                // the same name, or a second version of the root package, keeps none)
                if tests_of.map_or(true, |m| is_root && key == format!("{pkg}.{m}")) {
                    out.tests.extend(p.tests);
                    out.unbuilt_tests.extend(p.unbuilt_tests);
                }
            }
            Err(es) => build_errors.extend(es),
        }
    }
    if !build_errors.is_empty() {
        return Err((vec![], build_errors));
    }
    out.entry = entry;
    if !lib.qualify.is_empty() {
        names::map_types(&mut out, &mut |t| lib.qualify.apply(t));
    }
    Ok(out)
}

/// The names of the types a module declares.
fn declared_types(m: &fortis_names::project::Module) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    if let Some(parsed) = &m.parsed {
        for d in fortis_syntax::view::declarations(fortis_syntax::view::N::new(&parsed.root, &parsed.toks)) {
            if matches!(d.kind, "class" | "enum" | "error" | "interface" | "mixin" | "observation" | "type") {
                if let Some(n) = d.name {
                    out.insert(n);
                }
            }
        }
    }
    out
}

pub fn build_source(src: &str, file: &str, lib: &build::LibIndex) -> Result<Program, (Vec<Record>, Vec<build::BuildError>)> {
    let parsed = match fortis_syntax::parse_module(src) {
        Ok(p) => p,
        Err(_) => return Err((vec![Record::new("E-SYN-000", "the module does not parse", "LANG-02-01", file, 1, 1, fortis_diag::Json::obj([("expected", fortis_diag::Json::str("a module")), ("found", fortis_diag::Json::str(""))]))], vec![])),
    };
    let (records, built) = fortis_types::check::check_parsed_typed(&parsed, file, |ty, root| build::build_module(ty, root, "main", lib));
    let errors: Vec<Record> = records.into_iter().filter(|r| r.severity == "error").collect();
    if !errors.is_empty() {
        return Err((errors, vec![]));
    }
    built.map_err(|e| (vec![], e))
}

/// Check one library module (library/fortis/<m>.ft) under its module key and build the typed IR of its bodies.
pub fn build_library_module(root: &std::path::Path, m: &str, lib: &build::LibIndex) -> Result<Program, (Vec<Record>, Vec<build::BuildError>)> {
    let path = root.join("library").join("fortis").join(format!("{m}.ft"));
    let src = std::fs::read_to_string(&path).unwrap_or_default();
    let file = format!("library/fortis/{m}.ft");
    let parsed = match fortis_syntax::parse_module(&src) {
        Ok(p) => p,
        Err(_) => return Err((vec![], vec![build::BuildError { line: 0, message: format!("{file} does not parse") }])),
    };
    let module = format!("fortis.{m}");
    let (records, built) = fortis_types::check::check_module_typed(&parsed, &file, &module, |ty, root| build::build_module(ty, root, &module, lib));
    let errors: Vec<Record> = records.into_iter().filter(|r| r.severity == "error").collect();
    match built {
        Ok(p) => Ok(p),
        Err(e) => Err((errors, e)),
    }
}

/// The core library's typed IR, built once per process.
pub fn library_program(root: &std::path::Path, lib: &build::LibIndex) -> Program {
    let mut out = Program::default();
    for m in build::LIBRARY_MODULES {
        if let Ok(p) = build_library_module(root, m, lib) {
            out.functions.extend(p.functions);
            out.types.extend(p.types);
            out.consts.extend(p.consts);
        }
    }
    out
}

/// A user program linked with the library: the library's functions, types and constants follow the user's, so a
/// lookup by name finds the user's declaration first. A library type whose bare name the user already declared is
/// kept under `Name@module` (`Currency@fortis.money`), the name the builder uses for that standard enum (D-142).
pub fn link(mut user: Program, lib: &Program) -> Program {
    let have: std::collections::HashSet<String> = user.functions.iter().map(|f| f.name.clone()).collect();
    user.functions.extend(lib.functions.iter().filter(|f| !have.contains(&f.name)).cloned());
    let tys: std::collections::HashSet<String> = user.types.iter().map(|t| t.name.clone()).collect();
    let mut extra = Vec::new();
    for t in &lib.types {
        if !tys.contains(&t.name) {
            extra.push(t.clone());
        } else if !is_user_module(&t.module) {
            let mut c = t.clone();
            c.name = format!("{}@{}", t.name, t.module);
            if !tys.contains(&c.name) {
                extra.push(c);
            }
        }
    }
    user.types.extend(extra);
    user.consts.extend(lib.consts.iter().cloned());
    user
}
