"""DEL-46: the semantic query database (compiler/query-db).

`python3 -I conformance/tools/querydb_check.py [--json]` runs the database's evidence binary:

1. `edits`: 50 scripted edits of a module (body literals, bodies that change their summary, type errors, whitespace,
   added lines, signatures, tests, class methods, and their reversals). After each edit every pass of every unit is
   demanded. The pass queries that run must be exactly those an independent reference predicts: their own text, the
   module's interface, or a summary they read changed. Every value must equal the reference's, and the module's
   records a fresh check's.
2. `corruption`: ten corrupted caches; every corrupted entry is detected and recomputed, never served.
3. `cancel`: twenty module checks cancelled at random moments return within 50 ms and leave the database consistent.
4. `equality`: the database's records equal the batch check's for every corpus module (the equality DEL-43 records).
"""
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
BIN = os.path.join(ROOT, "compiler", "target", "release", "fortis-query-db")


def main(argv):
    workload = os.path.join(ROOT, "build", "bench", "check-50k", "m00000.ft")
    if not os.path.exists(workload):
        subprocess.run([sys.executable, "-I", os.path.join(ROOT, "benchmarks", "check_workload.py"), "--lines", "50000", "--out", os.path.dirname(workload)], check=True, capture_output=True)
    corpus = sorted(os.path.join(d, f) for d in ("conformance/programs", "conformance/corpus", "examples") for f in os.listdir(os.path.join(ROOT, d)) if f.endswith(".ft"))
    runs = [("edits", []), ("corruption", ["examples/apps/orders/src/orders.ft"]), ("cancel", [os.path.relpath(workload, ROOT)]), ("equality", corpus)]
    results, ok = {}, True
    for name, args in runs:
        r = subprocess.run([BIN, name] + args, capture_output=True, text=True, cwd=ROOT, timeout=7200)
        last = r.stdout.strip().splitlines()[-1] if r.stdout.strip() else r.stderr.strip()[-300:]
        results[name] = {"exit": r.returncode, "summary": last, "output": r.stdout}
        ok &= r.returncode == 0
        print(f"{'PASS' if r.returncode == 0 else 'FAIL'} {last}")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump(results, open(os.path.join(ROOT, "build", "querydb.json"), "w"), indent=1)
    print("query database check: " + ("OK" if ok else "FAILED"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
