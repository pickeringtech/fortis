"""Allocator probe self-test (SP-04): a noalloc body that allocates must abort under the probe; one that does
not must complete. Builds two tiny crates against the runtime and reports both exits."""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BUILD = os.path.join(ROOT, "build", "native")
TARGET = os.path.join(BUILD, "target")
CASES = {
    "probe_alloc": "fn main() { let n = fortis_rt::probe::noalloc(\"allocating\", || { let v: Vec<i64> = Vec::with_capacity(8); v.len() }); println!(\"{}\", n); }",
    "probe_clean": "fn main() { let n = fortis_rt::probe::noalloc(\"clean\", || { let a = 40i64; a + 2 }); println!(\"{}\", n); }",
}


def main():
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_NET_OFFLINE="true")
    ok = True
    for name, body in CASES.items():
        d = os.path.join(BUILD, name)
        os.makedirs(os.path.join(d, "src"), exist_ok=True)
        open(os.path.join(d, "Cargo.toml"), "w").write(f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nfortis_rt = {{ path = "{os.path.relpath(os.path.join(HERE, "runtime"), d)}" }}\n')
        open(os.path.join(d, "src", "main.rs"), "w").write("#![forbid(unsafe_code)]\n#[global_allocator]\nstatic ALLOC: fortis_rt::probe::Counting = fortis_rt::probe::Counting;\n" + body + "\n")
        b = subprocess.run(["cargo", "build", "-q", "--offline"], cwd=d, env=env, capture_output=True, text=True)
        if b.returncode != 0:
            print(name, "build failed", b.stderr[-300:])
            return 1
        r = subprocess.run([os.path.join(TARGET, "debug", name)], capture_output=True, text=True)
        expected_abort = name == "probe_alloc"
        aborted = r.returncode != 0
        print(f"{name}: exit {r.returncode} stderr {r.stderr.strip()[:120]!r} -> {'PASS' if aborted == expected_abort else 'FAIL'}")
        ok = ok and aborted == expected_abort
    print("probe self-test:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
