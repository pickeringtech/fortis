"""The shared IR validator (DEL-45) and the VM's refusal of IR it rejects (DEL-35 criterion 2).

1. Coverage: `fortisc validate --coverage` — for every C.1 row and C.3 pattern (the G1 rows and, since DEL-39, the
   asynchronous ones), the row's fixture (compiler/crates/fortis-validate/fixtures/<id>.ft) validates and the row's
   own check rejects its targeted corruption. The report is written to build/validator/coverage.json.
2. Soundness against the frontend: every corpus program the checker accepts and the builder lowers validates with
   no violation, and so does the core library.
3. The JSON form round-trips for every such program.
4. Determinism: the validator's report is identical over 20 runs in one process and across two processes.
5. The VM refuses each corrupted fixture rather than running it.

  python3 -I conformance/tools/validator_check.py [--json]"""
from __future__ import annotations

import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
FIX = os.path.join("compiler", "crates", "fortis-validate", "fixtures")


def run(args):
    return subprocess.run([FORTISC] + args, capture_output=True, text=True, cwd=ROOT, timeout=3600)


def main(argv):
    problems = []
    cov = json.loads(run(["validate", "--coverage"]).stdout)
    checks = cov["checks"]
    rows = [c for c in checks if not c["id"].startswith("wf.")]
    for c in rows:
        if not c.get("covered"):
            problems.append(f"coverage: {c['id']} not covered: {c.get('note') or c.get('valid_violations') or c.get('rejection')}")
    by = {s: sum(1 for c in rows if c["subset"] == s) for s in sorted({c["subset"] for c in rows})}
    print(f"coverage: {sum(c.get('covered', False) for c in rows)} of {len(rows)} C.1 rows and C.3 patterns covered ({', '.join(f'{n} {s}' for s, n in by.items())}), each corruption rejected by the row's own check (a whitelist pattern's by a C.3 check)")
    results = json.load(open(os.path.join(ROOT, "conformance", "results.json"), encoding="utf-8"))["results"]
    paths = sorted({r["path"] for r in results if r["path"].endswith(".ft") and (r["verdict"] in ("accept", "execute") or r.get("observed", "").startswith("accept"))})
    out = run(["validate"] + paths).stdout
    built = sum(1 for l in out.splitlines() if l.startswith("=== ")) - sum(1 for l in out.splitlines() if l.startswith("skipped"))
    viol = [l for l in out.splitlines() if l.startswith("violation")]
    problems += [f"corpus: {v}" for v in viol[:20]]
    lib = run(["validate", "--library", os.path.join(FIX, "C1.effects.ft")]).stdout
    libv = [l for l in lib.splitlines() if l.startswith("violation")]
    problems += [f"library: {v}" for v in libv[:20]]
    print(f"soundness: {built} corpus programs built, {len(viol)} violations; core library {len(libv)} violations")
    rt = run(["validate", "--roundtrip"] + paths).stdout.strip().splitlines()
    rt_bad = [l for l in rt if l.startswith("roundtrip")]
    problems += rt_bad[:10]
    print(f"round trip: {rt[-1] if rt else ''}")
    fixtures = sorted(os.path.join(FIX, f) for f in os.listdir(os.path.join(ROOT, FIX)) if f.endswith(".ft"))
    d1 = run(["validate", "--runs", "20"] + paths + fixtures).stdout.strip().splitlines()[-1]
    d2 = run(["validate", "--runs", "20"] + paths + fixtures).stdout.strip().splitlines()[-1]
    if d1 != d2 or "problems: 0" not in d1:
        problems.append(f"determinism: {d1} / {d2}")
    print(f"determinism: 20 runs per program in each of two processes: {d1}; second process {'identical' if d1 == d2 else 'DIFFERENT'}")
    refused = []
    for c in rows:
        if not c["id"].startswith("C1."):
            continue
        o = run(["vm", "--corrupt", c["id"], os.path.join(FIX, c["id"] + ".ft")]).stdout
        line = next((l for l in o.splitlines() if l.startswith("observed: ")), "")
        ok = line.startswith("observed: refused-ir:")
        refused.append({"check": c["id"], "refused": ok, "observed": line[len("observed: "):]})
        if not ok:
            problems.append(f"vm ran corrupted {c['id']}: {line}")
    print(f"VM refusals: {sum(r['refused'] for r in refused)} of {len(refused)} corrupted fixtures refused")
    for p in problems[:30]:
        print("PROBLEM", p)
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build", "validator"), exist_ok=True)
        json.dump(cov, open(os.path.join(ROOT, "build", "validator", "coverage.json"), "w"), indent=1)
        json.dump({"corpus_programs": built, "violations": viol, "library_violations": libv, "roundtrip": rt[-1:] , "determinism": [d1, d2], "vm_refusals": refused, "problems": problems},
                  open(os.path.join(ROOT, "build", "validator", "report.json"), "w"), indent=1)
    print("validator check:", "OK" if not problems else f"{len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
