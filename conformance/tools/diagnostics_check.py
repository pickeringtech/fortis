"""DEL-03 checks: schema validity of every record of a run, class inventory by identity against
Programme 7.1 (when the Delivery Programme, a maintainers' planning file, is in the checkout), one positive
golden and one negative record per class present, two renderings carrying the same set, and corrections that
apply and make the pinned fixture pass.
`--regenerate` rewrites the golden records from the pinned fixtures."""
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import diagnostics  # noqa: E402
import parser       # noqa: E402
import run          # noqa: E402
import diagnose     # noqa: E402
import planning     # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
GOLDEN = os.path.join(ROOT, "conformance", "diagnostics", "golden")
NEGATIVE = os.path.join(ROOT, "conformance", "diagnostics", "negative")
PINNED = {   # code -> fixture that produces exactly one record of that code
    "E-SYN-000": "conformance/programs/syntax_missing_return_type.ft",
    "E-SYN-001": "conformance/programs/syntax_semicolon.ft",
    "E-SYN-002": "conformance/programs/syntax_keyword_binding.ft",
    "E-SYN-004": "conformance/programs/syntax_logical_then_coalesce.ft",
}


def programme_classes():
    """The class inventory of Programme 7.1, or None in a checkout without the Delivery Programme."""
    dp = planning.optional(planning.PROGRAMME, "diagnostics", "the class inventory against Programme 7.1")
    if not dp:
        return None
    t = open(dp, encoding="utf-8").read()
    sec = t[t.index("### 7.1"):t.index("### 7.2")]
    return [r.split("|")[1].strip() for r in sec.splitlines() if r.startswith("| ") and not r.startswith("| Class") and not r.startswith("|---")]


def apply_edits(src, edits):
    lines = src.split("\n")
    for e in sorted(edits, key=lambda e: (e["line"], e["column"]), reverse=True):
        l = lines[e["line"] - 1]
        lines[e["line"] - 1] = l[:e["column"] - 1] + e["replacement"] + l[e["end_column"] - 1:]
    return "\n".join(lines)


def main(argv):
    errors = []
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    # 1. class inventory by identity
    classes = programme_classes()
    ours = [c["name"] for c in diagnostics.CLASSES]
    if classes is not None and classes != ours:
        errors.append(f"class inventory differs from Programme 7.1: {set(classes) ^ set(ours)}")
    # 2. produce records for the pinned fixtures; compare or regenerate goldens
    os.makedirs(GOLDEN, exist_ok=True)
    produced = {}
    for code, path in PINNED.items():
        recs = [r for r in diagnose.diagnose(rec, os.path.join(ROOT, path)) if r["code"] == code]
        if len(recs) != 1:
            errors.append(f"{path} produced {len(recs)} records of {code}, expected 1")
            continue
        produced[code] = recs[0]
        for e in diagnostics.validate(recs[0]):
            errors.append(f"{code} record invalid: {e}")
        gpath = os.path.join(GOLDEN, code + ".json")
        if "--regenerate" in argv or not os.path.exists(gpath):
            json.dump(recs[0], open(gpath, "w", encoding="utf-8"), indent=2, sort_keys=True)
            open(os.path.join(GOLDEN, code + ".txt"), "w", encoding="utf-8").write(diagnostics.render_text(recs[0]) + "\n")
        golden = json.load(open(gpath, encoding="utf-8"))
        if golden != recs[0]:
            errors.append(f"{code}: produced record differs from golden (regenerate deliberately with --regenerate)")
        if open(os.path.join(GOLDEN, code + ".txt"), encoding="utf-8").read().rstrip("\n") != diagnostics.render_text(recs[0]):
            errors.append(f"{code}: text rendering differs from golden")
    # 3. every record of the whole run validates
    jl = os.path.join(ROOT, "conformance", "diagnostics.jsonl")
    n = 0
    if os.path.exists(jl):
        for line in open(jl, encoding="utf-8"):
            r = json.loads(line)
            n += 1
            for e in diagnostics.validate(r):
                errors.append(f"run record {r.get('code')} at {r.get('file')}:{r.get('line')}: {e}")
    # 4. negatives: one per class, missing one mandatory field, must be rejected
    os.makedirs(NEGATIVE, exist_ok=True)
    for c in diagnostics.CLASSES:
        code = c["prefixes"][0] if c["prefixes"][0].count("-") == 2 else c["prefixes"][0] + "-001"
        fields = {f: f"<{f}>" for f in c["fields"][1:]}      # the first mandatory field is omitted
        neg = diagnostics.make(code, "negative record", "LANG-00-00", "x.ft", 1, 1, fields)
        neg["rule"] = "-"
        npath = os.path.join(NEGATIVE, c["name"].replace(" ", "_") + ".json")
        if "--regenerate" in argv or not os.path.exists(npath):
            json.dump(neg, open(npath, "w", encoding="utf-8"), indent=2, sort_keys=True)
        if not diagnostics.validate(json.load(open(npath, encoding="utf-8"))):
            errors.append(f"negative record for class {c['name']} was accepted")
        pos = diagnostics.make(code, "positive record", "LANG-00-00", "x.ft", 1, 1, {f: f"<{f}>" for f in c["fields"]})
        pos["rule"] = "-"
        if diagnostics.validate(pos):
            errors.append(f"positive record for class {c['name']} rejected: {diagnostics.validate(pos)}")
    # 5. two renderings carry the same set (code, file, line, column)
    recs = list(produced.values())
    text_set = set(re.findall(r"^(\S+):(\d+):(\d+): \S+ (\S+) ", "\n".join(diagnostics.render_text(r) for r in recs), re.M))
    json_set = {(r["file"], str(r["line"]), str(r["column"]), r["code"]) for r in recs}
    if text_set != json_set:
        errors.append("text and JSON renderings carry different (file, line, column, code) sets")
    tty = subprocess.run([sys.executable, "-I", os.path.join(ROOT, "conformance", "tools", "diagnose.py"), PINNED["E-SYN-001"]], capture_output=True, text=True, cwd=ROOT).stdout
    if not tty.startswith("{"):
        errors.append("diagnose.py on a pipe did not emit JSON lines")
    # 6. corrections apply and the pinned fixture then passes
    applied = 0
    for code, r in produced.items():
        if "correction" not in r:
            continue
        src = open(os.path.join(ROOT, PINNED[code]), encoding="utf-8").read()
        fixed = apply_edits(src, r["correction"]["edits"])
        ok, why, *_ = run.try_parse(rec, fixed)
        if not ok:
            errors.append(f"{code}: applying the correction does not make {PINNED[code]} parse: {why}")
        else:
            applied += 1
    match = "not checked" if classes is None else classes == ours
    print(f"classes: {len(ours) if classes is None else len(classes)} (identity match: {match}); golden records: {len(produced)}; run records validated: {n}; negatives rejected: {len(diagnostics.CLASSES)}; corrections applied and re-checked: {applied}")
    if errors:
        print("ERRORS:")
        for e in errors:
            print(" -", e)
        return 1
    print("diagnostics check: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
