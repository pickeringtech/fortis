"""DEL-44 (DW-06, PROG-DEL-04): the supported matrix is published and every entry has its evidence.

`python3 -I conformance/tools/matrix_check.py [--json]`

For docs/delivery/matrix.json:
- every entry is in exactly one class (Full, Representative, Unsupported) and Programme 16's six Language 1.0
  interaction cases are present and Full;
- Full: each scenario passes on each backend the entry lists, at this commit: a program fixture by its release
  results (conformance/release/results.json for the VM and native, conformance/execution/results.json for the
  interpreter's run, or the release results' interpreter judgement for a non-executing fixture), an acceptance
  application by its run log, `release-results:<backend>` by every linked fixture passing there, and
  `features-check:…` by features_check.py; each listed diagnostic fixture is rejected with its code by both checkers
  (its release results on the interpreter row, the reference checker, and the VM row, `fortisc check`);
- Representative: names a Full entry and carries a mapping record;
- Unsupported: its negative command, run now, produces the diagnostic.
"""
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
INTERACTIONS = ["noalloc+failure+capture", "pure+closure kinds", "async+defer+scope", "views+from+closures",
                "linear outcome in a linear container+early return", "ranged loop+refinement+checked arithmetic"]


def load(path):
    p = os.path.join(ROOT, path)
    return json.load(open(p, encoding="utf-8")) if os.path.exists(p) else {}


def main(argv):
    matrix = load("docs/delivery/matrix.json")
    release = {(r["path"], r["backend"]): r for r in load("conformance/release/results.json").get("results", [])}
    execution = {r["path"]: r for r in load("conformance/execution/results.json").get("results", [])}
    entries = {e["id"]: e for e in matrix.get("entries", [])}
    rows, problems = [], 0

    def expect(name, ok, detail=""):
        nonlocal problems
        rows.append({"check": name, "ok": bool(ok), "detail": str(detail)[:500]})
        problems += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok else f": {str(detail)[:300]}"))

    expect("every entry is in exactly one class", all(e.get("class") in ("Full", "Representative", "Unsupported") for e in entries.values()) and len(entries) == len(matrix.get("entries", [])))
    for i in INTERACTIONS:
        e = entries.get(f"interaction:{i}")
        expect(f"Programme 16 interaction `{i}` is Full", e is not None and e["class"] == "Full")

    def scenario_ok(s, backend):
        if s.startswith("release-results:"):
            be = s.split(":", 1)[1]
            rs = [r for (p, b), r in release.items() if b == be]
            return bool(rs) and all(r["status"] in ("PASS", "KNOWN") for r in rs), f"{sum(r['status'] == 'PASS' for r in rs)}/{len(rs)} pass"
        if s.startswith("features-check:"):
            r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "features_check.py")], capture_output=True, text=True, cwd=ROOT, timeout=3600)
            return r.returncode == 0, r.stdout.strip().splitlines()[-1] if r.stdout.strip() else r.stderr[-200:]
        if s.startswith("acceptance/"):
            log = load(os.path.join(s, "run-log.json"))
            spec = load(os.path.join(s, "spec.json"))
            ok = log.get("semantic_correctness", {}).get("passed") and backend in spec.get("backends", ["interpreter"])
            return ok, f"run log passed: {log.get('semantic_correctness', {}).get('passed')}; backends {spec.get('backends', ['interpreter'])}"
        if backend == "interpreter":
            r = execution.get(s) or release.get((s, "interpreter"))
        else:
            r = release.get((s, backend))
        return bool(r) and r["status"] == "PASS", (r or {}).get("observed", "no result")

    for e in entries.values():
        if e["class"] == "Full":
            for s in e.get("scenarios", []):
                for be in e.get("backends", []):
                    ok, why = scenario_ok(s, be)
                    expect(f"{e['id']}: {s} on {be}", ok, why)
            for d in e.get("diagnostics", []):
                ok = all(release.get((d, be), {}).get("status") == "PASS" for be in ("interpreter", "vm"))
                expect(f"{e['id']}: diagnostic {d} rejected with its code by both checkers", ok)
        elif e["class"] == "Representative":
            rep = entries.get(e.get("representative", ""))
            expect(f"{e['id']}: represented by a Full entry, with a mapping record", rep is not None and rep["class"] == "Full" and e.get("mapping"), e.get("representative"))
        else:
            cmd = e["negative"]["command"]
            r = subprocess.run([FORTISC] + cmd, capture_output=True, text=True, cwd=ROOT, timeout=1800)
            codes = sorted({json.loads(l)["code"] for l in r.stdout.splitlines() if l.startswith("{") and '"code"' in l})
            expect(f"{e['id']}: the negative fixture produces {e['diagnostic']}", r.returncode != 0 and e["diagnostic"] in codes, codes)
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "PROG-DEL-04", "entries": len(entries), "checks": rows}, open(os.path.join(ROOT, "build", "matrix.json"), "w"), indent=1)
    print(f"\n{len(entries)} entries ({', '.join(f'{c}: {sum(e['class'] == c for e in entries.values())}' for c in ('Full', 'Representative', 'Unsupported'))}); {len(rows)} checks, {sum(r['ok'] for r in rows)} pass")
    print("matrix check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
