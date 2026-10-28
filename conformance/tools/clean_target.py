"""Clean-target native execution and dependency inspection (DEL-40 criterion 4, PROG-FND-03).

`python3 -I conformance/tools/clean_target.py [--json]`

An ordinary production executable must not need the development VM, and must carry no executor, registry or
exporter that the program did not request. For each program below, `fortisc native` builds it and this tool
inspects the result:

1. **Dependencies**: `cargo tree --offline` of the generated crate lists only the crate itself, `fortis-native` and
   `fortis-runtime`; no VM, scheduler library, metrics registry or exporter.
2. **Linked code**: the binary's symbols (`nm -C`) contain nothing from `fortis_vm`, nothing from the allocator probe,
   and no exporter or registry. A synchronous program contains no executor code (`fortis_native::task`) at all, since
   it requested none.
3. **Threads**: the runtime source never creates an OS thread (no `thread::spawn`, `thread::Builder` or
   `thread::scope`), so every native program, pools included, runs on its main thread under the controlled executor.
4. **Execution**: the binary runs from a clean environment (`env -i`) with the program's expected output, and its
   standard error carries only the records the program asks for.
"""
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
BUILD = os.path.join(ROOT, "build", "native-prod")
RUNTIME = os.path.join(ROOT, "compiler", "crates", "fortis-native", "src")
ALLOWED = {"fortis-native", "fortis-runtime"}
# (program, requests an executor, the records its standard error may carry)
PROGRAMS = [
    ("conformance/programs/exec_host_console.ft", False, set()),
    ("conformance/programs/exec_while_mut.ft", False, set()),
    ("conformance/programs/exec_join_both.ft", True, set()),
    ("conformance/programs/exec_pool_parallel_await.ft", True, set()),
    ("conformance/programs/exec_host_supervisor.ft", True, {"task-completion"}),
]


def crate_of(path):
    stem = os.path.splitext(os.path.basename(path))[0]
    return "fx_" + "".join(c.lower() if c.isalnum() else "_" for c in stem)


def main(argv):
    rows, problems = [], 0
    threaded = []
    for root, _, files in os.walk(RUNTIME):
        for f in files:
            src = open(os.path.join(root, f), encoding="utf-8").read()
            for m in re.finditer(r"thread::(spawn|Builder|scope)", src):
                threaded.append(f"{f}: thread::{m.group(1)}")
    out = subprocess.run([FORTISC, "native"] + [p for p, _, _ in PROGRAMS], capture_output=True, text=True, cwd=ROOT, timeout=3600).stdout
    observed = dict(re.findall(r"^=== (.+)\n(?:.*\n)*?observed: (.*)$", out, re.M))
    for path, executor, records in PROGRAMS:
        crate = crate_of(path)
        diags = []
        tree = subprocess.run(["cargo", "tree", "--offline", "-e", "normal", "--prefix", "none"], capture_output=True, text=True,
                              cwd=os.path.join(BUILD, crate), env={**os.environ, "CARGO_TARGET_DIR": os.path.join(BUILD, "target")})
        crates = sorted({l.split()[0] for l in tree.stdout.splitlines() if l.strip()} - {crate})
        if tree.returncode != 0 or not set(crates) <= ALLOWED:
            diags.append(f"dependencies {crates} (cargo tree exit {tree.returncode})")
        exe = os.path.join(BUILD, "target", "release", crate)
        syms = subprocess.run(["nm", "-C", exe], capture_output=True, text=True).stdout
        count = lambda pat: len(re.findall(pat, syms))
        vm, probe, export, task = count(r"fortis_vm"), count(r"fortis_native::probe"), count(r"(?i)exporter|metrics::registry|telemetry"), count(r"fortis_native::task::")
        if vm or probe or export:
            diags.append(f"linked: fortis_vm {vm}, probe {probe}, exporter/registry {export}")
        if not executor and task:
            diags.append(f"a synchronous program links {task} executor symbols")
        run = subprocess.run(["env", "-i", exe], capture_output=True, text=True, timeout=120)
        kinds = {json.loads(l).get("kind") for l in run.stderr.splitlines() if l.startswith("{")}
        if not kinds <= records:
            diags.append(f"unrequested records on standard error: {sorted(kinds - records)}")
        want = observed.get(path, "").split("|")
        if run.stdout.splitlines() != want:
            diags.append(f"clean-environment output {run.stdout.splitlines()} differs from the harness run {want}")
        problems += bool(diags)
        rows.append({"program": path, "crate": crate, "dependencies": crates, "fortis_vm_symbols": vm, "probe_symbols": probe,
                     "exporter_symbols": export, "executor_symbols": task, "requests_executor": executor, "records": sorted(kinds), "ok": not diags, "diagnostics": diags})
        print(f"{'PASS' if not diags else 'FAIL'} {path}: dependencies {crates or ['none']}; executor symbols {task}; fortis_vm {vm}; records {sorted(kinds) or 'none'}"
              + ("" if not diags else "\n     " + "\n     ".join(diags)))
    if threaded:
        problems += 1
        print("FAIL the runtime creates threads: " + "; ".join(threaded))
    else:
        print("PASS the runtime source creates no OS thread")
    print(f"\nclean target: {len(rows)} programs, {sum(r['ok'] for r in rows)} need only fortis-native and fortis-runtime and carry nothing unrequested")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "PROG-FND-03", "programs": rows, "runtime_threads": threaded}, open(os.path.join(ROOT, "build", "clean-target.json"), "w"), indent=1)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
