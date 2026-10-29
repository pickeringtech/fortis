"""The documentation pilot's tests (pilot/ledger), in parts that can run at once.

    python3 -I conformance/tools/pilot_tests.py [--shard J/K]

The gate's pilot step runs `bin/fortis test pilot/ledger` (every module's tests on the definitional interpreter)
and `fortisc test pilot/ledger` (the VM). On the interpreter most of the time goes into setting up each module's
interpreter, one module after another, so this runs the same tests as `bin/fortis test pilot/ledger MODULE...`,
which checks the package and then runs the named modules' tests. With `--shard J/K` the modules are dealt out by
size, largest first, and part J runs; part 1 also runs the VM's tests. Every part must pass: exit 0, a summary
with no failure, and every test result a pass. Without `--shard` it runs every module, which is the gate's step.
"""
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
PILOT = "pilot/ledger"
FORTIS = os.path.join(ROOT, "bin", "fortis")
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")


def modules():
    src = os.path.join(ROOT, PILOT, "src")
    return {f[:-3]: os.path.getsize(os.path.join(src, f)) for f in sorted(os.listdir(src)) if f.endswith(".ft")}


def part(mods, j, k):
    load, owner = [0] * k, {}
    for m in sorted(mods, key=lambda m: (-mods[m], m)):
        i = min(range(k), key=lambda s: (load[s], s))
        owner[m] = i + 1
        load[i] += mods[m]
    return sorted(m for m, i in owner.items() if i == j)


def judge(label, cmd):
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
    recs = []
    for line in (r.stdout + "\n" + r.stderr).splitlines():
        if line.startswith("{"):
            try:
                recs.append(json.loads(line))
            except ValueError:
                pass
    results = [x for x in recs if x.get("kind") == "test-result"]
    failing = [x for x in results if x.get("status") != "pass"]
    summary = [x for x in recs if x.get("kind") == "test-summary"]
    ok = r.returncode == 0 and not failing and all(x.get("failed", 0) == 0 for x in summary)
    print(f"{'PASS' if ok else 'FAIL'} {label}: exit {r.returncode}, {len(results)} test results, {len(failing)} not passing")
    for x in failing[:10]:
        print(f"    {x.get('test')} / {x.get('case')}: {x.get('status')}")
    if not ok and r.returncode != 0:
        print("\n".join("    " + l for l in (r.stdout + r.stderr).strip().splitlines()[-10:]))
    return ok


def main(argv):
    j, k = 1, 1
    if "--shard" in argv:
        spec = argv[argv.index("--shard") + 1]
        if not re.fullmatch(r"[1-9]\d*/[1-9]\d*", spec) or int(spec.split("/")[0]) > int(spec.split("/")[1]):
            print("pilot tests: --shard takes J/K with 1 <= J <= K")
            return 2
        j, k = (int(x) for x in spec.split("/"))
    mods = modules()
    mine = part(mods, j, k) if k > 1 else sorted(mods)
    ok = True
    if mine:
        ok &= judge(f"interpreter, modules {', '.join(mine)}", [sys.executable, "-I", FORTIS, "test", PILOT] + mine + ["--format", "json"])
    if j == 1:
        ok &= judge("VM, every module", [FORTISC, "test", "--format", "json", PILOT])
    print("pilot tests: " + ("OK" if ok else "FAILED") + (f" (part {j}/{k})" if k > 1 else ""))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
