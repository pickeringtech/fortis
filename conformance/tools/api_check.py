"""DEL-43 criterion 3 (LANG-11-05): weakening an exported function's written modifiers is reported by `check`.

`python3 -I conformance/tools/api_check.py [--json]`

Each fixture directory of `conformance/api` holds `old.ft` and `new.ft`. `fortisc check --api-baseline old.ft new.ft
--json` must report an `E-EFF-040` warning on exactly the lines of `new.ft` marked `// expect: E-EFF-040` and nothing
else; each record must be schema-valid (diagnostics.validate) and the check must still succeed (a warning is not an
error). Both versions must check without records on their own.
"""
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import diagnostics  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
API = os.path.join(ROOT, "conformance", "api")


def main(argv):
    rows, problems = [], 0
    for d in sorted(x for x in os.listdir(API) if os.path.isdir(os.path.join(API, x))):
        old, new = os.path.join(API, d, "old.ft"), os.path.join(API, d, "new.ft")
        want = sorted(i for i, l in enumerate(open(new, encoding="utf-8").read().split("\n"), start=1) if re.search(r"//\s*expect:\s*E-EFF-040", l))
        clean = all(subprocess.run([FORTISC, "check", p], capture_output=True, text=True).returncode == 0 for p in (old, new))
        r = subprocess.run([FORTISC, "check", "--json", "--api-baseline", old, new], capture_output=True, text=True)
        recs = [json.loads(l) for l in r.stdout.splitlines() if l.startswith("{")]
        got = sorted(x["line"] for x in recs if x["code"] == "E-EFF-040")
        invalid = [e for x in recs for e in diagnostics.validate(x)]
        ok = clean and r.returncode == 0 and got == want and len(recs) == len(got) and not invalid and all(x["severity"] == "warning" for x in recs)
        problems += not ok
        rows.append({"fixture": d, "expected_lines": want, "reported_lines": got, "records": recs, "schema_errors": invalid, "ok": ok})
        print(f"{'PASS' if ok else 'FAIL'} {d}: expected E-EFF-040 at {want or 'no line'}, reported at {got or 'no line'}" + ("" if clean else " (a version has records on its own)") + (f" schema: {invalid}" if invalid else ""))
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "LANG-11-05", "fixtures": rows}, open(os.path.join(ROOT, "build", "api.json"), "w"), indent=1)
    print("api check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
