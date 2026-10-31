"""Appendix C.2 over the corpus (DEL-39, LANG-07-09, X-126): every unsupported ownership shape is rejected before
lowering with the code the table lists for it.

`python3 -I conformance/tools/shapes_check.py [--json]`

The table's rows are read from the Language Reference, so a new row without a fixture fails here. Each row's fixture
must be a `// reject:` program whose header names the row's code. The production checker (`fortisc check`) must
report that code at the marked line, and the reference checker's recorded result
(conformance/checker/results.json, written by checker_check.py) must be PASS with the same code; a syntax
rejection is the reference parser's (conformance/results.json, written by run.py).
"""
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
LR = os.path.join(ROOT, "docs", "Fortis_Language_Reference.md")
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
CHECKER_RESULTS = os.path.join(ROOT, "conformance", "checker", "results.json")
SYNTAX_RESULTS = os.path.join(ROOT, "conformance", "results.json")
# the fixture of each C.2 row, keyed by the row's shape text
FIXTURES = {
    "a loan returned `from` two parameters": "conformance/programs/own_shape_two_origins.ft",
    "a class that stores a loan (a lifetime parameter, or a field of a loan or loan-carrying type)": ["conformance/programs/own_shape_stored_loan.ft", "conformance/programs/own_lifetime_param.ft"],
    "a loan stored in a collection, `Shared` or `Mutex`": ["conformance/programs/own_shape_loan_in_shared.ft", "conformance/programs/own_shape_loan_in_mutex.ft", "conformance/programs/own_shape_unsupported.ft"],
    "a `fn mut` closure escaping into a `Shared`": "conformance/programs/own_shape_mut_closure_in_shared.ft",
    "a future holding a loan launched on a parallel pool": "conformance/programs/asy_pool_loan_bad.ft",
    "a self-referential class": "conformance/programs/own_shape_self_referential.ft",
    "a partial move": "conformance/programs/own_adv_partial_move.ft",
}


def table():
    """The C.2 rows: (shape, code)."""
    src = open(LR, encoding="utf-8").read()
    section = src[src.index("### C.2"):src.index("### C.3")]
    rows = []
    for line in section.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) == 3 and cells[0] not in ("Shape", "---"):
            m = re.match(r"`(E-[A-Z]+-\d+)`", cells[1])
            rows.append((cells[0], m.group(1) if m else None))
    return rows


def main(argv):
    ref = {r["path"]: r for r in json.load(open(CHECKER_RESULTS, encoding="utf-8"))["results"]}
    syntax = {r["path"]: r for r in json.load(open(SYNTAX_RESULTS, encoding="utf-8"))["results"] if r.get("backend") == "syntax"}
    out, problems = [], 0
    for shape, code in table():
        fixtures = FIXTURES.get(shape)
        fixtures = [fixtures] if isinstance(fixtures, str) else (fixtures or [])
        diags = [] if code else ["the row names no code"]
        if not fixtures:
            diags.append("no fixture")
        for f in fixtures:
            src = open(os.path.join(ROOT, f), encoding="utf-8").read()
            head = re.match(r"//\s*reject:\s*(E-[A-Z]+-\d+)", src)
            if not head or head.group(1) != code:
                diags.append(f"{f}: header names {head.group(1) if head else 'no rejection'}, the table {code}")
            marked = [i + 1 for i, l in enumerate(src.splitlines()) if re.search(r"//\s*reject:\s*E-", l) and not l.lstrip().startswith("//")]
            r = subprocess.run([FORTISC, "check", "--format", "text", f], capture_output=True, text=True, cwd=ROOT, timeout=600)
            found = {(int(m.group(1)), m.group(2)) for m in re.finditer(r"^[^:]+:(\d+):\d+: error (E-[A-Z]+-\d+)", r.stdout + r.stderr, re.M)}
            if not any(c == code and (not marked or ln in marked) for ln, c in found):
                diags.append(f"{f}: fortisc check reports {sorted(found)}, not {code} at line {marked}")
            rr = (syntax if code.startswith("E-SYN") else ref).get(f)
            if rr is None or rr.get("status") != "PASS" or code not in (rr.get("observed") or ""):
                diags.append(f"{f}: the reference checker's result is {rr.get('status') if rr else 'missing'} ({rr.get('observed') if rr else ''})")
        problems += bool(diags)
        out.append({"shape": shape, "code": code, "fixtures": fixtures, "ok": not diags, "diagnostics": diags})
        print(f"{'PASS' if not diags else 'FAIL'} {code or '?':<10} {shape}" + ("" if not diags else "\n     " + "\n     ".join(diags)))
    print(f"\nC.2: {len(out)} shapes, {sum(r['ok'] for r in out)} rejected before lowering with their listed code by both checkers")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "LANG-07-09", "shapes": out}, open(os.path.join(ROOT, "build", "shapes.json"), "w"), indent=1)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
