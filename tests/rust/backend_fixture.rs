//! Unit fixture: typed syntax is supplied in memory; library declarations are compiled into the test.
//! This calls the declaration/typing passes only to prepare the MIR builder's input, never a driver or linker.
use fortis_mir::{
    build::{build_module, LibIndex},
    Program,
};
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
pub fn build(src: &str) -> Program {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn_scoped(scope, || build_here(src))
            .unwrap()
            .join()
            .unwrap()
    })
}
fn build_here(src: &str) -> Program {
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
    assert!(
        ty.dc.records.is_empty(),
        "unit input rejected: {:?}",
        ty.dc
            .records
            .iter()
            .map(|r| (&r.code, &r.message))
            .collect::<Vec<_>>()
    );
    build_module(&mut ty, root, "main", &LibIndex::default()).expect("unit lowering")
}
pub fn source(d: &fortis_testkit::Json) -> String {
    let s = |key| d.get(key).unwrap().as_str().unwrap();
    let params = d
        .get("params")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .map(|p| {
            format!(
                "{}: {}",
                p.get("name").unwrap().as_str().unwrap(),
                p.get("type").unwrap().as_str().unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{}\n{}func value({params}) {} {}\n",
        s("prelude"),
        s("modifiers"),
        s("type"),
        if s("kind") == "expr" {
            format!("=> {}", s("source"))
        } else {
            s("source").to_string()
        }
    )
}
