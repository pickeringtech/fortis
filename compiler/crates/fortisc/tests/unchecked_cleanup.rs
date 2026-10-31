//! The VM does not rely on the checker for a class value's `cleanup` to run exactly once (LR 7.9, 12.2; D-348).
//!
//! Each program in tests/unchecked/ is one a checker defect let run: a loan that outlives its owner's destruction, or
//! two bindings that own one value. It is built whatever the checker reports and run on the VM, which must print its
//! `// backstop:` lines: the owner's destruction runs the cleanup, a loan's end does not, and a value already
//! destroyed is not destroyed again. A program that reads what a `return` moved out stops with a dynamic check
//! failure, the last `backstop:` item (`dynamic:…`), never silently (D-391), and the observed text joins the lines
//! printed before the failure, as the interpreter's does (D-398). conformance/tools/execute_check.py runs
//! the same programs on the interpreter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("the repository root")
}

/// The program's observed text on the VM, built from its typed IR without the checker's verdict: its console
/// lines, then the dynamic check failure or defect that stopped it, if one did (D-398: the observed text joins
/// the lines printed before the failure, as the interpreter's does).
fn run_unchecked(root: &Path, file: &str, src: &str) -> String {
    let lib = fortis_mir::build::LibIndex::load(root);
    let core = fortis_mir::library_program(root, &lib);
    let parsed = fortis_syntax::parse_module(src).unwrap_or_else(|_| panic!("{file} parses"));
    let (_records, built) = fortis_types::check::check_parsed_typed(&parsed, file, |ty, r| fortis_mir::build::build_module(ty, r, "main", &lib));
    let prog = fortis_mir::link(built.unwrap_or_else(|e| panic!("{file} builds: {e:?}")), &core);
    fortis_vm::run_main(&prog, file, &[], &HashMap::new()).text
}

/// The checker and the IR builder recurse over the program, and a test thread's stack (2 MB unless RUST_MIN_STACK
/// says otherwise) is too small for them in a debug build, where `fortisc` itself runs them on the main thread's
/// 8 MB: the test runs on a thread of its own with room to spare.
const TEST_STACK: usize = 64 << 20;

#[test]
fn cleanup_runs_once_without_the_checker() {
    std::thread::Builder::new()
        .name("unchecked-cleanup".into())
        .stack_size(TEST_STACK)
        .spawn(cleanup_runs_once)
        .expect("spawn the test thread")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

fn cleanup_runs_once() {
    let root = root();
    let dir = root.join("compiler/crates/fortisc/tests/unchecked");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).expect("tests/unchecked").map(|e| e.expect("an entry").path()).filter(|p| p.extension().map_or(false, |x| x == "ft")).collect();
    files.sort();
    assert!(!files.is_empty(), "tests/unchecked holds the programs");
    for path in files {
        let src = std::fs::read_to_string(&path).expect("the program");
        let want = src.lines().next().and_then(|l| l.strip_prefix("// backstop: ")).unwrap_or_else(|| panic!("{} starts with `// backstop:`", path.display()));
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        let got = run_unchecked(&root, &file, &src);
        assert_eq!(got, want, "{file} on the VM");
    }
}
