//! An aggregate destroys the values of its own that sit where its type has a loan, and only those (LR 7.9; D-366).
//!
//! A tuple or record element whose type is a loan can hold a value of the aggregate's own: a `??` whose fallback is
//! moved, an `if` that moves a value in, a binding assigned a value of its own. The conformance fixtures
//! (conformance/programs/exec_loan_owned_part*.ft) cover the shapes every backend builds. Each program in
//! tests/owned_part/ holds shapes native code does not build yet (a record holding a loan, D-360; an `if` giving a
//! loan or a moved value, D-361) or traces differently (D-344). `fortisc vm` checks and runs it, and must print its
//! `// execute:` lines, which are the interpreter's, and its `// execute-trace:` events.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("the repository root")
}

/// The value of a `// NAME:` marker line.
fn marker<'a>(src: &'a str, name: &str) -> &'a str {
    let prefix = format!("// {name}:");
    src.lines().find_map(|l| l.strip_prefix(prefix.as_str())).map(str::trim).unwrap_or_else(|| panic!("a `// {name}:` line"))
}

#[test]
fn owned_parts_are_destroyed_with_the_aggregate() {
    let root = root();
    let dir = root.join("compiler/crates/fortisc/tests/owned_part");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).expect("tests/owned_part").map(|e| e.expect("an entry").path()).filter(|p| p.extension().map_or(false, |x| x == "ft")).collect();
    files.sort();
    assert!(!files.is_empty(), "tests/owned_part holds the programs");
    for path in files {
        let src = std::fs::read_to_string(&path).expect("the program");
        let out = Command::new(env!("CARGO_BIN_EXE_fortisc")).arg("vm").arg(&path).current_dir(&root).output().expect("fortisc runs");
        let text = String::from_utf8_lossy(&out.stdout);
        let line = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)).map(|s| s.trim().to_string()).unwrap_or_else(|| panic!("{}: no `{name}` line in\n{text}", path.display()));
        assert_eq!(line("observed:"), marker(&src, "execute"), "{} on the VM", path.display());
        assert_eq!(line("trace:"), marker(&src, "execute-trace"), "{}'s trace on the VM", path.display());
    }
}
