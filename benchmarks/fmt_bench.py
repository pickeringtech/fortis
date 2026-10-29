"""PROG-NFR-01: warm parse and format of 1k ordinary lines, over a 1,000-edit session (DEL-43 criterion 7).

`python3 -I benchmarks/fmt_bench.py [--json]`

Conditions, workload, edits and the figure are those of DEL-43's preregistration: the module
`check_workload.module(0, 13)` (refused if its digest differs from the preregistered one, when the preregistration,
a maintainers' planning file, is in the checkout), 1,000 cumulative edits
from a seeded stream, five passes in one warm process (`compiler/crates/fortis-fmt/examples/edit_bench.rs`), and
the pooled p95 against the SHOULD budget of 10 ms.
"""
import hashlib
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, HERE)
import check_workload  # noqa: E402

PREREGISTRATION = os.path.join(HERE, "PREREGISTRATION-DEL-43.md")
WORKLOAD_SHA256 = (open(PREREGISTRATION, encoding="utf-8").read().split("module sha256 `")[1].split("`")[0]
                   if os.path.exists(PREREGISTRATION) else None)
BUDGET_MS = 10.0


def main(argv):
    text = check_workload.module(0, 13)
    digest = hashlib.sha256(text.encode()).hexdigest()
    if WORKLOAD_SHA256 is None:
        print(f"fmt-bench: skipped comparing the workload's digest with the preregistered one: "
              f"{os.path.relpath(PREREGISTRATION, ROOT)} is a maintainers' planning file and is not in this checkout", file=sys.stderr)
    elif digest != WORKLOAD_SHA256:
        print(f"the workload's digest {digest} is not the preregistered {WORKLOAD_SHA256}")
        return 1
    path = os.path.join(ROOT, "build", "bench", "fmt-1k.ft")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w", encoding="utf-8").write(text)
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "fortis-fmt", "--example", "edit_bench"], cwd=os.path.join(ROOT, "compiler"), check=True)
    exe = os.path.join(ROOT, "compiler", "target", "release", "examples", "edit_bench")
    r = subprocess.run([exe, path, "--passes", "5", "--edits", "1000"], capture_output=True, text=True, timeout=3600)
    if r.returncode != 0:
        print(r.stderr)
        return 1
    res = json.loads(r.stdout)
    p = res["pooled"]
    met = p["p95_ms"] <= BUDGET_MS
    for i, s in enumerate(res["passes"], start=1):
        print(f"pass {i}: p50 {s['p50_ms']:.3f} ms, p95 {s['p95_ms']:.3f} ms, p99 {s['p99_ms']:.3f} ms, max {s['max_ms']:.3f} ms")
    print(f"pooled over {p['edits']} edits of a {res['lines']}-line module: p50 {p['p50_ms']:.3f} ms, p95 {p['p95_ms']:.3f} ms, p99 {p['p99_ms']:.3f} ms, "
          f"mean {p['mean_ms']:.3f} ms, max {p['max_ms']:.3f} ms; budget p95 <= {BUDGET_MS:.0f} ms {'met' if met else 'NOT met'} "
          "(development machine, not a dedicated runner)")
    if "--json" in argv:
        json.dump({"rule": "PROG-NFR-01", "workload_sha256": digest, "budget_ms": BUDGET_MS, "met": met, **res},
                  open(os.path.join(ROOT, "build", "bench", "fmt-results.json"), "w"), indent=1)
    return 0 if met else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
