//! fortis-ir: the frontend's intermediate forms. The declaration IR (every declaration with its member types and
//! signatures, as the checker sees them) and the control-flow graph of each body (labelled edges between blocks of
//! statements, with the places each block reads or assigns).

pub mod cfg;
pub mod decls;
pub mod function;

use fortis_names::{Libraries, Library};
use fortis_syntax::view::{declarations, N};
use fortis_syntax::Parsed;
use fortis_types::decls::{DeclChecker, Session};

/// The declaration IR of one module checked against the reference library.
pub fn decl_ir(parsed: &Parsed, file: &str) -> String {
    let libs = Libraries { libs: vec![Library::new()] };
    let sess = Session::new(&libs);
    let mut dc = DeclChecker::new(&sess, 0, file, "main", "");
    dc.load(N::new(&parsed.root, &parsed.toks), false);
    dc.check();
    decls::dump(&dc)
}

/// The CFG text of every block-bodied function of a module, in declaration order.
pub fn cfgs(parsed: &Parsed) -> Vec<(String, String)> {
    let root = N::new(&parsed.root, &parsed.toks);
    declarations(root)
        .into_iter()
        .filter(|d| d.kind == "func")
        .filter_map(|d| {
            let name = d.name.clone().unwrap_or_default();
            cfg::build(d.node, &name).map(|g| (name, cfg::dump(&g)))
        })
        .collect()
}
