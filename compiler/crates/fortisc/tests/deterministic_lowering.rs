//! The native lowering is deterministic: the same program lowers to byte-identical Rust in every run (D-241).
//!
//! Each run is a separate `fortisc native --emit-only` process, so each has its own hash seeds; the second takes the
//! programs in the reverse order, so no state carried from one program to the next can change another's output.
//! A difference in the generated crate would also change a failing build's rustc error, and with it the digest the
//! documentation check records for a known defect.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// docs/guide/08-functions.md#16, a known native failure (D-55): its rustc error names a line of the generated Rust.
const CLOSURES_IN_A_LIST: &str = r#"// accept
import fortis.host { Host }

func applyAll(steps: List[dynamic fn(Int) Int], start: Int) Int {
  let mut value = start
  for step in steps {
    value = step(value)
  }
  return value
}

export func main(args: List[String]) Void {
  let caps = Host.capabilities()
  let bonus = 10
  let double: dynamic fn(Int) Int = fn(n: Int) Int => Int.saturatingMul(n, 2)
  let addBonus: dynamic fn(Int) Int = fn[bonus](n: Int) Int => Int.saturatingAdd(n, bonus)
  let steps = List.of(move double, move addBonus)
  caps.console.print("${applyAll(steps, 5)}")
}
"#;

/// Corpus programs with classes, enums and matches, closures, flow-sensitive references and a task pool.
const CORPUS: &[&str] = &[
    "conformance/corpus/LR-2-01.ft",
    "conformance/programs/ref_flow_paths_ok.ft",
    "conformance/programs/match_patterns_ok.ft",
    "conformance/programs/asp_user_order.ft",
    "conformance/programs/asy_pool_shareable_ok.ft",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("the repository root")
}

/// Every emitted crate's files, by crate directory, from one `fortisc native --emit-only` process.
fn emit(root: &Path, programs: &[PathBuf]) -> BTreeMap<String, (String, String)> {
    let out = Command::new(env!("CARGO_BIN_EXE_fortisc")).arg("native").args(programs).arg("--emit-only").current_dir(root).output().expect("fortisc runs");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut crates = BTreeMap::new();
    for dir in text.lines().filter_map(|l| l.strip_prefix("observed: emitted ")) {
        let dir = Path::new(dir);
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_else(|e| panic!("{}: {e}", dir.join(f).display()));
        crates.insert(dir.display().to_string(), (read("Cargo.toml"), read("src/main.rs")));
    }
    assert_eq!(crates.len(), programs.len(), "every program is emitted; fortisc printed:\n{text}{}", String::from_utf8_lossy(&out.stderr));
    crates
}

#[test]
fn the_same_program_lowers_to_the_same_rust() {
    let root = root();
    // copies under names of their own, so no other check's crate of the same name is read or overwritten
    let dir = root.join("build").join("determinism");
    std::fs::create_dir_all(&dir).unwrap();
    let mut programs = vec![];
    let mut add = |name: &str, src: &str| {
        let p = dir.join(format!("determinism_{name}.ft"));
        std::fs::write(&p, src).unwrap();
        programs.push(p);
    };
    add("closures_in_a_list", CLOSURES_IN_A_LIST);
    for c in CORPUS {
        let stem = Path::new(c).file_stem().unwrap().to_string_lossy().to_lowercase().replace('-', "_");
        add(&stem, &std::fs::read_to_string(root.join(c)).unwrap());
    }
    let first = emit(&root, &programs);
    programs.reverse();
    let second = emit(&root, &programs);
    for (krate, (toml, rust)) in &first {
        let (toml2, rust2) = &second[krate];
        assert!(toml == toml2, "{krate}: Cargo.toml differs between two runs");
        if rust != rust2 {
            let line = rust.lines().zip(rust2.lines()).position(|(a, b)| a != b).map_or(rust.lines().count().min(rust2.lines().count()), |i| i) + 1;
            panic!("{krate}: src/main.rs differs between two runs, first at line {line}");
        }
    }
}
