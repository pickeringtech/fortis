//! In-memory declaration and typing fixture. No driver, linker or backend runs.
use fortis_names::library::{Libraries, Library, LibraryModule};
use fortis_syntax::{view::N, Parsed};
use fortis_types::{
    decls::{DeclChecker, Session},
    typer::Typer,
};
use std::sync::{Arc, OnceLock};
fn modules() -> &'static Vec<(&'static str, Arc<Parsed>)> {
    static MODULES: OnceLock<Vec<(&str, Arc<Parsed>)>> = OnceLock::new();
    MODULES.get_or_init(|| {
        [
            ("prelude", include_str!("../../library/fortis/prelude.ft")),
            ("text", include_str!("../../library/fortis/text.ft")),
            (
                "collections",
                include_str!("../../library/fortis/collections.ft"),
            ),
            ("numeric", include_str!("../../library/fortis/numeric.ft")),
            ("time", include_str!("../../library/fortis/time.ft")),
            ("money", include_str!("../../library/fortis/money.ft")),
            ("host", include_str!("../../library/fortis/host.ft")),
        ]
        .into_iter()
        .map(|(name, src)| (name, Arc::new(fortis_syntax::parse_module(src).unwrap())))
        .collect()
    })
}
pub fn check(src: &str) -> Vec<String> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn_scoped(scope, || check_here(src))
            .unwrap()
            .join()
            .unwrap()
    })
}
fn check_here(src: &str) -> Vec<String> {
    let parsed = fortis_syntax::parse_module(src).expect("unit syntax");
    let modules = modules()
        .iter()
        .map(|(name, p)| {
            let key = format!("fortis.{name}");
            let mut module = LibraryModule::new(&key, "fortis", p.clone(), None);
            module.path = key.clone();
            (key, Arc::new(module))
        })
        .collect();
    let libs = Libraries {
        libs: vec![Library { modules }],
    };
    let sess = Session::new(&libs);
    let root = N::new(&parsed.root, &parsed.toks);
    let mut dc = DeclChecker::new(&sess, 0, "unit.ft", "main", "");
    dc.load(root, false);
    dc.check();
    let mut ty = Typer::new(dc);
    ty.record_types = true;
    ty.run(root);
    let mut codes: Vec<_> = ty.dc.records.iter().map(|r| r.code.to_string()).collect();
    codes.sort();
    codes.dedup();
    codes
}
