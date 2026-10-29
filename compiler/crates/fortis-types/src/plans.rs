//! Plan compatibility (Language Reference 11.4; PROG-VM-04; LANG-11-03): a build-time step, separate from type
//! checking, whose records name the policy site.
//!
//! `noalloc` is a claim under a plan. The core's plan is the observation a function carries: a `noalloc` function
//! with `@observe(Schema)` emits that schema's record at every exit, and a schema field whose type allocates (it is
//! not trivial: a `String`, a collection, a class holding one) makes the record allocate. That conflict is
//! `E-EFF-030` at the `@observe` site. Type checking never varies with the plan, so `check` does not run this step;
//! `build`, `vm`, `native` and `test` run it before producing or running anything.

use fortis_diag::{Json, Record};
use fortis_syntax::Parsed;

use crate::typer::Typer;
use crate::types::strip;

/// The plan-compatibility records of one typed module context: the module's declarations as its context loaded
/// them, so a package module sees the packages visible to it and an imported type (a sibling's alias of `Int`)
/// resolves exactly as the checker resolves it.
fn plan_records(ty: &Typer) -> Vec<Record> {
    let mut out = Vec::new();
    // `@observe` is the prelude's aspect only where this module's table resolves the name to it: an aspect the
    // module declares or imports under that name attaches no core observation, and the IR builder records none
    // for it (fortis-mir `core_aspect`), so it has no plan to conflict with (11.8; D-314)
    if !ty.dc.table.get("observe").map_or(true, |d| d.module == "fortis.prelude") {
        return out;
    }
    for (_, d) in ty.dc.table.decls.iter() {
        if d.kind != "func" || d.module != ty.dc.module_name {
            continue;
        }
        let Some(sig) = d.sig.as_ref() else { continue };
        if !ty.expanded_mods(sig).contains("noalloc") {
            continue;
        }
        for a in &d.aspects {
            let ts = a.direct_tokens();
            if ts.get(1).map(|t| t.text.as_str()) != Some("observe") {
                continue;
            }
            let schema = a.find("argList").and_then(|al| al.tokens().into_iter().find(|t| t.text.starts_with(|c: char| c.is_uppercase()))).map(|t| t.text.clone());
            let Some(schema) = schema else { continue };
            let Some(sd) = ty.dc.table.get(&schema) else { continue };
            // the schema's field types were written in the module that declares it: an imported observation's
            // names (an alias of Int) are in that module's table, not in the caller's
            let module = sd.module.clone();
            let foreign = if module == ty.dc.module_name {
                None
            } else {
                let loaded = ty.dc.sess.loaded.borrow();
                loaded.iter().find(|((_, p), _)| p == &module).map(|(_, t)| std::rc::Rc::clone(t))
            };
            let allocating: Vec<String> = sd.obs_fields.iter().filter(|(_, (ft, _))| {
                let ft = strip(ft);
                match &foreign {
                    Some(table) => !ty.dc.is_trivial_in(ft, table),
                    None => !ty.dc.is_trivial(ft),
                }
            }).map(|(f, (ft, _))| format!("{f}: {}", strip(ft).show())).collect();
            if let Some(first) = allocating.first() {
                let at = ts[0];
                out.push(Record::new(
                    "E-EFF-030",
                    format!(
                        "`{}` is declared `noalloc`, but the observation it carries ({schema}) records an allocating field ({first}): the plan conflicts with the claim at this policy site; record a trivial field, or drop `noalloc`",
                        d.name
                    ),
                    "LANG-11-03",
                    &ty.dc.file,
                    at.line,
                    at.col,
                    Json::obj([("required_contract", Json::str("noalloc")), ("violating_effect", Json::str("alloc")), ("witness_path", Json::str(&format!("@observe({schema}) -> {first}")))]),
                ));
            }
        }
    }
    out
}

/// The plan-compatibility records of one module in its own context (no package visible to it).
pub fn check(parsed: &Parsed, file: &str) -> Vec<Record> {
    let (_, out) = crate::check::check_parsed_with(parsed, file, |ty| plan_records(ty));
    out
}

/// The plan-compatibility records of a file or of every module of a package directory.
pub fn check_path(p: &std::path::Path) -> Vec<Record> {
    if p.is_dir() {
        // each module of the package sees exactly the packages visible to it, as `check` gives them: a standalone
        // module's context cannot see a sibling's types (an alias of `Int`) and would wrongly hold its fields
        // allocating. The root package's modules are the step's scope, as this directory's own src/ is. A package
        // that does not load falls to the per-file walk below (its manifest's records belong to `check`).
        let search = p.parent().unwrap_or(std::path::Path::new("."));
        if let Ok(ps) = fortis_names::resolve::PackageSet::load(p, search) {
            let root = ps.root().identity();
            let (_, planned) = crate::check::check_package_set_typed(&ps, |ty, _, _, pkg| {
                if pkg.identity() == root {
                    plan_records(ty)
                } else {
                    Vec::new()
                }
            });
            return planned.into_iter().flat_map(|(_, r)| r).collect();
        }
    }
    let mut files = Vec::new();
    if p.is_dir() {
        fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            if let Ok(rd) = std::fs::read_dir(d) {
                let mut es: Vec<_> = rd.flatten().map(|e| e.path()).collect();
                es.sort();
                for e in es {
                    if e.is_dir() {
                        walk(&e, out);
                    } else if e.extension().map_or(false, |x| x == "ft") {
                        out.push(e);
                    }
                }
            }
        }
        walk(&p.join("src"), &mut files);
    } else {
        files.push(p.to_path_buf());
    }
    let mut out = Vec::new();
    for f in files {
        let src = fortis_di::source(std::path::Path::new(&f)).unwrap_or_default();
        if let Ok(parsed) = fortis_syntax::parse_module(&src) {
            out.extend(check(&parsed, &f.display().to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(src: &str) -> Vec<String> {
        let parsed = fortis_syntax::parse_module(src).expect("parse");
        check(&parsed, "t.ft").into_iter().map(|r| r.code).collect()
    }

    /// The plan step judges the core `@observe` only, resolved by identity (D-314): a module's own aspect named
    /// `observe` attaches no core observation, so a `noalloc` function carrying it has no plan conflict.
    #[test]
    fn user_aspect_named_observe_has_no_plan() {
        let own = include_str!("../../../../conformance/aspects/user_observe_plan.ft");
        assert!(codes(own).is_empty(), "{:?}", codes(own));
        let core = include_str!("../../../../conformance/programs/eff_noalloc_observe.ft");
        assert_eq!(codes(core), vec!["E-EFF-030".to_string()]);
    }
}
