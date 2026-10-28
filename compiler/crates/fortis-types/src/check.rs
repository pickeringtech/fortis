//! The checker's entry points (checker/check.py): a module, or every module of a package set.

use std::path::Path;
use std::sync::Arc;

use fortis_diag::Record;
use fortis_names::project::Package;
use fortis_names::resolve::{rel, PackageSet};
use fortis_names::{Libraries, Library, LibraryModule, Resolver};
use fortis_syntax::view::N;
use fortis_syntax::Parsed;

use crate::decls::{DeclChecker, Session};
use crate::typer::Typer;

/// Names, declarations and visibility, then the typed walk of every body.
pub fn check_tree<'a>(sess: &'a Session<'a>, lib: usize, mtree: N<'a>, file: &str, module: &str, package: &str, entry: Option<&str>) -> Vec<Record> {
    check_tree_ir(sess, lib, mtree, file, module, package, entry).0
}

/// One module of a package set, with the function IR of its final pass.
pub fn check_tree_ir<'a>(sess: &'a Session<'a>, lib: usize, mtree: N<'a>, file: &str, module: &str, package: &str, entry: Option<&str>) -> (Vec<Record>, Vec<crate::journal::FnIr>) {
    // a module of a package: the manifest's `entry` judges its `main` (resolve.rs `check_entry`), not this check
    let mut records = Resolver::new(&sess.libs.libs[lib], file, false).resolve_module(mtree);
    let mut dc = DeclChecker::new(sess, lib, file, module, package);
    dc.entry_module = entry.map(|e| e.to_string());
    dc.load(mtree, false);
    dc.check();
    let mut ty = Typer::new(dc);
    ty.run(mtree);
    let ir = std::mem::take(&mut ty.ir_out);
    records.extend(ty.dc.records);
    (records, ir)
}

/// One module checked against the reference library.
pub fn check_parsed(parsed: &Parsed, file: &str) -> Vec<Record> {
    check_parsed_with(parsed, file, |_| ()).0
}

/// One module checked against the reference library, with access to the typer after its final pass.
pub fn check_parsed_with<R, F: for<'b> FnOnce(&Typer<'b>) -> R>(parsed: &Parsed, file: &str, f: F) -> (Vec<Record>, R) {
    let libs = Libraries { libs: vec![Library::new()] };
    let sess = Session::new(&libs);
    let mtree = N::new(&parsed.root, &parsed.toks);
    // a module checked on its own is its package's entry module (LR 4.6, D-51.2)
    let mut records = Resolver::new(&sess.libs.libs[0], file, true).resolve_module(mtree);
    let mut dc = DeclChecker::new(&sess, 0, file, "main", "");
    dc.load(mtree, false);
    dc.check();
    let mut ty = Typer::new(dc);
    ty.run(mtree);
    let r = f(&ty);
    records.extend(ty.dc.records);
    (records, r)
}

/// One module checked with the type of every expression recorded, then handed to `f` mutably (the typed IR
/// builder runs there).
pub fn check_parsed_typed<R, F: for<'b> FnOnce(&mut Typer<'b>, N<'b>) -> R>(parsed: &Parsed, file: &str, f: F) -> (Vec<Record>, R) {
    check_module_typed(parsed, file, "main", f)
}

/// As `check_parsed_typed`, for a module with the given module key (a library module: `fortis.prelude`).
pub fn check_module_typed<R, F: for<'b> FnOnce(&mut Typer<'b>, N<'b>) -> R>(parsed: &Parsed, file: &str, module: &str, f: F) -> (Vec<Record>, R) {
    let libs = Libraries { libs: vec![Library::new()] };
    let sess = Session::new(&libs);
    let mtree = N::new(&parsed.root, &parsed.toks);
    // the module key "main" is a module checked on its own; a library or package module is not an entry (D-51.2)
    let mut records = Resolver::new(&sess.libs.libs[0], file, module == "main").resolve_module(mtree);
    let mut dc = DeclChecker::new(&sess, 0, file, module, "");
    dc.load(mtree, false);
    dc.check();
    let mut ty = Typer::new(dc);
    ty.record_types = true;
    ty.run(mtree);
    let r = f(&mut ty, mtree);
    records.extend(ty.dc.records);
    (records, r)
}

#[cfg(test)]
mod closure_tests {
    use super::*;

    fn records(src: &str) -> Vec<Record> {
        let parsed = fortis_syntax::parse_module(src).expect("parse");
        check_parsed(&parsed, "t.ft")
    }

    fn typed(src: &str) -> Vec<(String, usize, String)> {
        records(src).into_iter().filter(|r| r.code.starts_with("E-")).map(|r| (r.code, r.line, r.rule)).collect()
    }

    #[test]
    fn dynamic_fn_accepts_distinct_lambdas() {
        let src = r#"
func applyAll(steps: List[dynamic fn(Int) Int], start: Int) Int {
    let mut value = start
    for step in steps {
        value = step(value)
    }
    return value
}

class Step {
    export run: dynamic fn(Int) Int
}

func run(b: Bool) Int {
    let mut f: dynamic fn(Int) Int = fn(x: Int) Int => x
    f = fn(x: Int) Int => Int.saturatingAdd(x, 1)
    let double: dynamic fn(Int) Int = fn(n: Int) Int => Int.saturatingMul(n, 2)
    let add: dynamic fn(Int) Int = fn(n: Int) Int => Int.saturatingAdd(n, 1)
    let steps = List.of(move double, move add)
    let listed: List[dynamic fn(Int) Int] = List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1))
    let chosen: dynamic fn(Int) Int = if b { give fn(x: Int) Int => x } else { give fn(x: Int) Int => Int.saturatingMul(x, 2) }
    let a = Step(run: fn(x: Int) Int => x)
    let c = Step(run: fn(x: Int) Int => Int.saturatingMul(x, 2))
    discard f
    discard chosen
    discard a
    discard c
    return applyAll(steps, applyAll(listed, 1))
}
"#;
        let got: Vec<_> = typed(src).into_iter().filter(|r| r.0 == "E-TYP-040").collect();
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn second_lambda_is_rejected() {
        let src = r#"
func run() Int {
    let mut bump = fn(x: Int) Int => x
    bump = fn(x: Int) Int => Int.saturatingAdd(x, 1)
    let xs = List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1))
    let first = Step(run: fn(x: Int) Int => x)
    let second = Step(run: fn(x: Int) Int => Int.saturatingAdd(x, 1))
    discard xs
    discard first
    discard second
    return 0
}

class Step {
    export run: fn(Int) Int
}
"#;
        let got = typed(src);
        let mut hits: Vec<_> = got.iter().map(|(c, line, rule)| (c.clone(), *line, rule.clone())).collect();
        hits.sort();
        assert_eq!(
            hits,
            vec![
                ("E-TYP-040".to_string(), 4, "LANG-08-04".to_string()),
                ("E-TYP-040".to_string(), 5, "LANG-08-04".to_string()),
                ("E-TYP-040".to_string(), 7, "LANG-08-04".to_string()),
            ],
            "{got:?}"
        );
    }

    #[test]
    fn optional_dynamic_and_literal_positions_accept_two_lambdas() {
        let src = r#"
func pick(b: Bool) Int {
    let f: (dynamic fn(Int) Int)? = if b { give fn(x: Int) Int => x } else { give fn(x: Int) Int => Int.saturatingAdd(x, 1) }
    let xs: List[dynamic fn(Int) Int]? = List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1))
    let r: {fs: List[dynamic fn(Int) Int]} = { fs: List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1)) }
    let pair: (List[dynamic fn(Int) Int], Int) = (List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingMul(x, 2)), 1)
    discard f
    discard xs
    discard r
    discard pair
    return 0
}
"#;
        let got: Vec<_> = typed(src).into_iter().filter(|r| r.0 == "E-TYP-040").collect();
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn null_binding_second_lambda_is_lang_08_04() {
        let src = r#"
func run() Int {
    let mut f: (fn(Int) Int)? = null
    f = fn(x: Int) Int => x
    f = fn(x: Int) Int => Int.saturatingAdd(x, 1)
    let mut g: (fn(Int) Int)? = fn(x: Int) Int => x
    g = fn(x: Int) Int => Int.saturatingAdd(x, 1)
    discard f
    discard g
    return 0
}
"#;
        let got = typed(src);
        let mut hits: Vec<_> = got.iter().map(|(c, line, rule)| (c.clone(), *line, rule.clone())).collect();
        hits.sort();
        assert_eq!(
            hits,
            vec![
                ("E-TYP-040".to_string(), 5, "LANG-08-04".to_string()),
                ("E-TYP-040".to_string(), 7, "LANG-08-04".to_string()),
            ],
            "{got:?}"
        );
    }

    fn typed_in_package(src: &str, package: &str) -> Vec<(String, usize, String)> {
        let parsed = fortis_syntax::parse_module(src).expect("parse");
        let libs = Libraries { libs: vec![Library::new()] };
        let sess = Session::new(&libs);
        let mtree = N::new(&parsed.root, &parsed.toks);
        check_tree(&sess, 0, mtree, "app.ft", "app", package, Some("app"))
            .into_iter()
            .filter(|r| r.code.starts_with("E-"))
            .map(|r| (r.code, r.line, r.rule))
            .collect()
    }

    #[test]
    fn two_classes_in_one_module_keep_distinct_pins() {
        let src = r#"
export class A {
    export run: fn(Int) Int
}
export class B {
    export run: fn(Int) Int
}
export func main(args: List[String]) Void {
    discard args
    let a = A(run: fn(x: Int) Int => x)
    let b = B(run: fn(x: Int) Int => Int.saturatingAdd(x, 1))
    discard a
    discard b
}
"#;
        let got = typed_in_package(src, "fun.closure.classes@0.1.0");
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn spread_of_a_different_class_is_a_second_lambda() {
        let src = r#"
class A {
    export run: fn(Int) Int
}
class B {
    export run: fn(Int) Int
}
func run() Int {
    let a = A(run: fn(x: Int) Int => x)
    let b = B(run: fn(x: Int) Int => Int.saturatingAdd(x, 1))
    let c = B(...move a)
    discard b
    discard c
    return 0
}
"#;
        let got = typed(src);
        let mut hits: Vec<_> = got.iter().map(|(c, line, rule)| (c.clone(), *line, rule.clone())).collect();
        hits.sort();
        assert_eq!(hits, vec![("E-TYP-040".to_string(), 11, "LANG-08-04".to_string())], "{got:?}");
    }

    #[test]
    fn spread_back_into_the_same_class_and_into_a_fresh_class_accept() {
        let same = r#"
class A {
    export run: fn(Int) Int
}
func same() Int {
    let a = A(run: fn(x: Int) Int => x)
    let again = A(...move a)
    discard again
    return 0
}
"#;
        let fresh = r#"
class A {
    export run: fn(Int) Int
}
class B {
    export run: fn(Int) Int
}
func into_fresh() Int {
    let a = A(run: fn(x: Int) Int => x)
    let into = B(...move a)
    discard into
    return 0
}
"#;
        // one check each: a second construction of A in the same check is a second lambda
        for src in [same, fresh] {
            let got: Vec<_> = typed(src).into_iter().filter(|r| r.0 == "E-TYP-040").collect();
            assert!(got.is_empty(), "{got:?}");
        }
    }

    #[test]
    fn nested_optional_dynamic_fn_accepts_two_lambdas() {
        let src = r#"
func optionalElements() List[(dynamic fn(Int) Int)?] {
    let xs: List[(dynamic fn(Int) Int)?] = List.of(null, fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1))
    return move xs
}
func nestedList() (List[dynamic fn(Int) Int]?)? {
    let xs: (List[dynamic fn(Int) Int]?)? = List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1))
    return move xs
}
func nestedRecord() ({fs: List[dynamic fn(Int) Int]}?)? {
    let r: ({fs: List[dynamic fn(Int) Int]}?)? = {fs: List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1))}
    return move r
}
"#;
        let got: Vec<_> = typed(src).into_iter().filter(|r| r.0 == "E-TYP-040").collect();
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn expected_result_does_not_bind_a_closure_type() {
        // P6 fills a generic associated call's type parameters from the expected result before the
        // arguments (D-253). A function type is left unbound (8.4): the first lambda pins it, and
        // `dynamic fn` is seeded afterwards. An enum element still binds, so `List.of(Red)` sees `Light`.
        let rejected = r#"
func run() Int {
    let ys: List[fn(Int) Int] = List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1))
    discard ys
    return 0
}
"#;
        let got = typed(rejected);
        let hits: Vec<_> = got.iter().map(|(c, line, rule)| (c.clone(), *line, rule.clone())).collect();
        assert_eq!(hits, vec![("E-TYP-040".to_string(), 3, "LANG-08-04".to_string())], "{got:?}");

        let dynamic = r#"
func run() Int {
    let ys: List[dynamic fn(Int) Int] = List.of(fn(x: Int) Int => x, fn(x: Int) Int => Int.saturatingAdd(x, 1))
    discard ys
    return 0
}
"#;
        assert!(typed(dynamic).is_empty(), "{:?}", typed(dynamic));

        let picked = r#"
enum Light {
    Red
    Green
}
enum Paint {
    Red
    Blue
}
func lights() Int {
    let xs: List[Light] = List.of(Red)
    discard xs
    return 1
}
"#;
        assert!(typed(picked).is_empty(), "{:?}", typed(picked));

        let ambiguous = r#"
enum Light {
    Red
    Green
}
enum Paint {
    Red
    Blue
}
func loose() Int {
    let xs = List.of(Red)
    return 1
}
"#;
        let names: Vec<_> = typed(ambiguous).into_iter().filter(|r| r.0 == "E-NAM-001").map(|r| (r.0, r.2)).collect();
        assert_eq!(names, vec![("E-NAM-001".to_string(), "LANG-04-05".to_string())], "{:?}", typed(ambiguous));
    }
}

/// Every module of every package in the set, each seeing exactly the packages visible to it.
pub fn check_package_set(ps: &PackageSet) -> Vec<Record> {
    check_package_set_ir(ps).0
}

/// Note every unannotated module constant before any module is typed, so an importer sees the initialiser's
/// type whichever module is checked first (D-54.4). Each round can see what the previous round noted. The seed's
/// records are discarded; the real pass reports them.
fn seed_inferred_consts<'a>(sess: &'a Session<'a>, ps: &'a PackageSet, ids: &[String], entry: Option<&str>) {
    let mut n = 0usize;
    for (_, p) in &ps.packages {
        n += p.modules.len();
    }
    for _ in 0..n.max(1) {
        let before = sess.inferred_consts.borrow().len();
        for (_, p) in &ps.packages {
            let lib = ids.iter().position(|i| *i == p.identity()).unwrap();
            for (mpath, m) in &p.modules {
                let Some(parsed) = &m.parsed else { continue };
                // the nodes stay in `ps`, which outlives the session that stores the loaded tables
                let mtree = N::new(&parsed.root, &parsed.toks);
                let mut dc = DeclChecker::new(sess, lib, &rel(&m.file), &format!("{}.{}", p.name, mpath), &p.identity());
                dc.entry_module = entry.map(|e| e.to_string());
                dc.load(mtree, false);
                let mut ty = Typer::new(dc);
                ty.seed_consts(mtree);
            }
        }
        if sess.inferred_consts.borrow().len() == before {
            break;
        }
    }
}

/// A module's function IR: its file, its parse and one entry per body typed in the final pass.
pub type ModuleIr = (String, Arc<Parsed>, Vec<crate::journal::FnIr>);

/// Every module of every package in the set, with each module's function IR.
pub fn check_package_set_ir(ps: &PackageSet) -> (Vec<Record>, Vec<ModuleIr>) {
    let ids: Vec<String> = ps.packages.iter().map(|(_, p)| p.identity()).collect();
    let lib_of = |p: &Package| ids.iter().position(|i| *i == p.identity()).unwrap();
    let mut libs: Vec<Library> = ids.iter().map(|_| Library::new()).collect();
    for (pi, (_, p)) in ps.packages.iter().enumerate() {
        let li = lib_of(p);
        let mut visible: Vec<usize> = ps.visible_packages(pi).into_iter().map(|(_, i)| i).collect();
        if !visible.contains(&pi) {
            visible.push(pi);
        }
        for qi in visible {
            let q = &ps.packages[qi].1;
            for (mpath, m) in &q.modules {
                let parsed: Arc<Parsed> = match &m.parsed {
                    Some(p) => p.clone(),
                    None => continue,
                };
                let lm = Arc::new(LibraryModule::new(&m.file.display().to_string(), &q.identity(), parsed, Some(lib_of(q))));
                libs[li].modules.insert(format!("{}.{}", q.name, mpath), lm.clone());
                if q.root.as_deref() == Some(mpath.as_str()) {
                    libs[li].modules.insert(q.name.clone(), lm);
                }
            }
        }
    }
    let libs = Libraries { libs };
    let sess = Session::new(&libs);
    let entry = ps.entry_key();
    seed_inferred_consts(&sess, ps, &ids, entry.as_deref());
    let mut out = Vec::new();
    let mut irs = Vec::new();
    for (_, p) in &ps.packages {
        for (mpath, m) in &p.modules {
            let parsed = match &m.parsed {
                Some(p) => p,
                None => continue,
            };
            let key = format!("{}.{}", p.name, mpath);
            let (recs, ir) = check_tree_ir(&sess, lib_of(p), N::new(&parsed.root, &parsed.toks), &rel(&m.file), &key, &p.identity(), entry.as_deref());
            out.extend(recs);
            irs.push((rel(&m.file), parsed.clone(), ir));
        }
    }
    (out, irs)
}

/// Every module of every package in the set with the type of every expression recorded, each handed to `f`
/// with its module key (`acme.stock.ids`): the typed IR builder runs there, module by module.
pub fn check_package_set_typed<R, F: for<'b> FnMut(&mut Typer<'b>, N<'b>, &str, &Package) -> R>(ps: &PackageSet, mut f: F) -> (Vec<Record>, Vec<(String, R)>) {
    let ids: Vec<String> = ps.packages.iter().map(|(_, p)| p.identity()).collect();
    let lib_of = |p: &Package| ids.iter().position(|i| *i == p.identity()).unwrap();
    let mut libs: Vec<Library> = ids.iter().map(|_| Library::new()).collect();
    for (pi, (_, p)) in ps.packages.iter().enumerate() {
        let li = lib_of(p);
        let mut visible: Vec<usize> = ps.visible_packages(pi).into_iter().map(|(_, i)| i).collect();
        if !visible.contains(&pi) {
            visible.push(pi);
        }
        for qi in visible {
            let q = &ps.packages[qi].1;
            for (mpath, m) in &q.modules {
                let parsed: Arc<Parsed> = match &m.parsed {
                    Some(p) => p.clone(),
                    None => continue,
                };
                let lm = Arc::new(LibraryModule::new(&m.file.display().to_string(), &q.identity(), parsed, Some(lib_of(q))));
                libs[li].modules.insert(format!("{}.{}", q.name, mpath), lm.clone());
                if q.root.as_deref() == Some(mpath.as_str()) {
                    libs[li].modules.insert(q.name.clone(), lm);
                }
            }
        }
    }
    let libs = Libraries { libs };
    let sess = Session::new(&libs);
    let entry = ps.entry_key();
    seed_inferred_consts(&sess, ps, &ids, entry.as_deref());
    let mut out = Vec::new();
    let mut built = Vec::new();
    for (_, p) in &ps.packages {
        for (mpath, m) in &p.modules {
            let parsed = match &m.parsed {
                Some(p) => p,
                None => continue,
            };
            let key = format!("{}.{}", p.name, mpath);
            let file = rel(&m.file);
            let mtree = N::new(&parsed.root, &parsed.toks);
            let mut records = Resolver::new(&sess.libs.libs[lib_of(p)], &file, false).resolve_module(mtree);
            let mut dc = DeclChecker::new(&sess, lib_of(p), &file, &key, &p.identity());
            dc.entry_module = entry.clone();
            dc.load(mtree, false);
            dc.check();
            let mut ty = Typer::new(dc);
            ty.record_types = true;
            ty.run(mtree);
            let r = f(&mut ty, mtree, &key, p);
            records.extend(ty.dc.records);
            out.extend(records);
            built.push((key, r));
        }
    }
    (out, built)
}

/// A package directory: the resolver's records, else every module's.
pub fn check_package_dir(dir: &Path, search: &Path) -> Vec<Record> {
    match PackageSet::load(dir, search) {
        Err(e) => vec![fortis_names::resolve::manifest_record(&e, &rel(&dir.join("project.yaml")))],
        Ok(mut ps) => {
            let r = ps.run();
            if !r.is_empty() {
                return r;
            }
            check_package_set(&ps)
        }
    }
}
