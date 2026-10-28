"""Corrections of the production frontend (DEL-03 criterion 4 extended to every correction-bearing class; DEL-32,
DEL-48, DEL-34 criterion 3).

Every record `fortisc check` emits with a `correction` is applied to its fixture (diagnostics_check.apply_edits) and
the edited program is checked again. A correction *works* when the record it repairs is gone (no record of its code
at its line) and no record of another code appears. For every class whose Programme 7.1 row names a correction, at
least one pinned fixture produces exactly one record whose correction leaves a program with no records at all; the
pinned fixtures are the corpus's own where one exists, else conformance/corrections/<class>.ft.

  python3 -I conformance/tools/corrections_check.py [--json]"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")


def check(paths):
    r = subprocess.run([FORTISC, "check", "--json"] + paths, capture_output=True, text=True, cwd=ROOT, timeout=3600)
    out, cur = {}, None
    for line in r.stdout.splitlines():
        if line.startswith("=== "):
            cur = line[4:]
            out[cur] = []
        elif line.startswith("{"):
            out[cur].append(json.loads(line))
    return out


def main(argv):
    import diagnostics
    from diagnostics_check import apply_edits
    syntax = json.load(open(os.path.join(ROOT, "conformance", "results.json"), encoding="utf-8"))["results"]
    paths = [r["path"] for r in syntax if r["verdict"] != "illustrative" and "statements" not in (r.get("detail") or "") and r["status"] in ("PASS", "INFO")]
    paths += sorted(os.path.join("conformance", "corrections", f) for f in os.listdir(os.path.join(ROOT, "conformance", "corrections")) if f.endswith(".ft"))
    before = check(paths)
    tmp = tempfile.mkdtemp(prefix="fortis-corrections-")
    trials = []
    for path, recs in before.items():
        for i, rec in enumerate(recs):
            if "correction" not in rec:
                continue
            src = open(os.path.join(ROOT, path), encoding="utf-8").read()
            fixed = apply_edits(src, [e for e in rec["correction"]["edits"] if e["file"] == path])
            dst = os.path.join(tmp, f"{len(trials)}.ft")
            open(dst, "w", encoding="utf-8").write(fixed)
            trials.append((path, rec, recs, dst))
    after = check([t[3] for t in trials])
    results, by_class = [], {}
    for path, rec, recs, dst in trials:
        recs2 = after.get(dst, [])
        cls = diagnostics.class_of(rec["code"])["name"]
        same = [x for x in recs2 if x["code"] == rec["code"] and x["line"] == rec["line"] and x["column"] == rec["column"]]
        before_here = {(x["code"], x["column"]) for x in recs if x["line"] == rec["line"]}
        new_here = [x for x in recs2 if x["line"] == rec["line"] and diagnostics.class_of(x["code"])["name"] == cls and (x["code"], x["column"]) not in before_here]
        works = not same and not new_here
        clean = works and not recs2 and len(recs) == 1
        results.append({"fixture": path, "code": rec["code"], "line": rec["line"], "kind": rec["correction"]["kind"], "works": works, "clean": clean, "after": sorted({f"{x['code']}@{x['line']}" for x in recs2})})
        b = by_class.setdefault(cls, {"corrections": 0, "work": 0, "pinned": None})
        b["corrections"] += 1
        b["work"] += works
        if clean and b["pinned"] is None:
            b["pinned"] = f"{path} ({rec['code']}: {rec['correction']['kind']})"
    # the Telemetry kit's diagnostics come from the plan validator (telemetry/plan.py), on the plan corpus; a
    # correction there inserts lines, so it works when no record of its code remains
    sys.path.insert(0, ROOT)
    from telemetry.registry import Registry
    from telemetry import plan as planmod
    reg = Registry()
    for prog in ("orders_v1.ft", "orders.ft"):
        reg.add_program([os.path.join(ROOT, "conformance", "telemetry", prog)])

    def plan_records(text):
        try:
            planmod.load(text, reg)
            return []
        except planmod.PlanInvalid as e:
            return [d.to_json() for d in e.diags]

    plans = os.path.join("conformance", "telemetry", "plans")
    for f in sorted(os.listdir(os.path.join(ROOT, plans))):
        if not f.endswith(".yaml"):
            continue
        path = os.path.join(plans, f)
        text = open(os.path.join(ROOT, path), encoding="utf-8").read()
        recs = plan_records(text)
        for rec in recs:
            if not rec.get("correction"):
                continue
            recs2 = plan_records(apply_edits(text, [dict(e, file=path) for e in rec["correction"]["edits"]]))
            works = not any(x["code"] == rec["code"] for x in recs2)
            clean = works and not recs2 and len(recs) == 1
            results.append({"fixture": path, "code": rec["code"], "line": rec["line"], "kind": rec["correction"]["kind"], "works": works, "clean": clean, "after": sorted({f"{x['code']}@{x['line']}" for x in recs2})})
            b = by_class.setdefault(diagnostics.class_of(rec["code"])["name"], {"corrections": 0, "work": 0, "pinned": None})
            b["corrections"] += 1
            b["work"] += works
            if clean and b["pinned"] is None:
                b["pinned"] = f"{path} ({rec['code']}: {rec['correction']['kind']})"
    with_correction = [c["name"] for c in diagnostics.CLASSES if c["correction"]]
    missing = [c for c in with_correction if not by_class.get(c, {}).get("pinned")]
    broken = [r for r in results if not r["works"]]
    for r in broken[:30]:
        print(f"DOES NOT WORK {r['fixture']}:{r['line']} {r['code']} ({r['kind']}) -> {r['after']}")
    for c in with_correction:
        b = by_class.get(c)
        if b:
            print(f"{c:<22} {b['corrections']:3} corrections, {b['work']:3} work; pinned: {b['pinned'] or 'NONE'}")
        else:
            print(f"{c:<22}   0 corrections; pinned: NONE")
    print(f"\n{len(results)} corrections applied, {len(results) - len(broken)} work; {len(with_correction) - len(missing)}/{len(with_correction)} correction-bearing classes pinned")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build", "rust-frontend"), exist_ok=True)
        json.dump({"results": results, "classes": by_class, "missing": missing}, open(os.path.join(ROOT, "build", "rust-frontend", "corrections.json"), "w"), indent=1)
    return 1 if (missing or broken) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
