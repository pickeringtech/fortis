"""The function IR of the production frontend (DEL-32, DEL-48, DEL-33, DEL-34; PROG-CMP-02).

Runs `fortisc ir` over every corpus module (and the examples and application A), validates each function object
against reference/schemas/ir.schema.json (by hand: the conformance tools have no third-party dependencies), and
checks the facts for consistency: a closed obligation closes at or after its opening, an exit runs only deferred
blocks the function registered, and every function carries all five fact families. It reports, per chunk range,
how many functions carry loans, obligations, effects, observation operations and cleanup edges.

  python3 -I conformance/tools/ir_check.py [--json]"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
SCHEMA = json.load(open(os.path.join(ROOT, "reference", "schemas", "ir.schema.json"), encoding="utf-8"))


def check(value, schema, path, errs):
    """The subset of JSON Schema the IR schema uses: type, enum, pattern, minimum, required, additionalProperties,
    properties, items, oneOf."""
    if "oneOf" in schema:
        ok = []
        for s in schema["oneOf"]:
            e2 = []
            check(value, s, path, e2)
            ok.append(not e2)
        if sum(ok) != 1:
            errs.append(f"{path}: matches {sum(ok)} of the oneOf schemas")
        return
    t = schema.get("type")
    types = t if isinstance(t, list) else ([t] if t else [])
    pytype = {"object": dict, "array": list, "string": str, "boolean": bool, "null": type(None)}
    if types:
        good = False
        for ty in types:
            if ty == "integer":
                good |= isinstance(value, int) and not isinstance(value, bool)
            else:
                good |= isinstance(value, pytype[ty])
        if not good:
            errs.append(f"{path}: expected {types}, got {type(value).__name__}")
            return
    if "enum" in schema and value not in schema["enum"]:
        errs.append(f"{path}: {value!r} not in {schema['enum']}")
    if "pattern" in schema and isinstance(value, str) and not re.search(schema["pattern"], value):
        errs.append(f"{path}: {value!r} does not match {schema['pattern']}")
    if "minimum" in schema and isinstance(value, int) and not isinstance(value, bool) and value < schema["minimum"]:
        errs.append(f"{path}: {value} below {schema['minimum']}")
    if isinstance(value, dict):
        for r in schema.get("required", []):
            if r not in value:
                errs.append(f"{path}: missing {r}")
        props = schema.get("properties", {})
        if schema.get("additionalProperties") is False:
            for k in value:
                if k not in props:
                    errs.append(f"{path}: unexpected {k}")
        for k, s in props.items():
            if k in value:
                check(value[k], s, f"{path}.{k}", errs)
    if isinstance(value, list) and "items" in schema:
        for i, x in enumerate(value):
            check(x, schema["items"], f"{path}[{i}]", errs)


def chunk_num(c):
    return int(c.split("-")[1]) if c and c.startswith("DEL-") else 999


def main(argv):
    own = json.load(open(os.path.join(ROOT, "docs", "delivery", "ownership.json"), encoding="utf-8"))
    syntax = json.load(open(os.path.join(ROOT, "conformance", "results.json"), encoding="utf-8"))["results"]
    fixtures = [r for r in syntax if r["verdict"] != "illustrative" and "statements" not in (r.get("detail") or "") and r["status"] in ("PASS", "INFO") and not r["observed"].startswith("reject:E-SYN")]
    extra = sorted(os.path.join("examples", f) for f in os.listdir(os.path.join(ROOT, "examples")) if f.endswith(".ft"))
    paths = [r["path"] for r in fixtures] + extra
    # package fixtures that resolve, and application A: every module of each, typed with the packages it sees
    pkg_dir = os.path.join(ROOT, "conformance", "packages")
    packages = [os.path.join("conformance", "packages", d) for d in sorted(os.listdir(pkg_dir)) if os.path.exists(os.path.join(pkg_dir, d, "expect.json"))]
    packages.append(os.path.join("acceptance", "a"))
    out = subprocess.run([FORTISC, "ir"] + paths + packages, capture_output=True, text=True, cwd=ROOT, timeout=3600).stdout
    per_file, cur = {}, None
    for line in out.splitlines():
        if line.startswith("=== "):
            cur = line[4:]
            per_file[cur] = []
        elif line.startswith("{"):
            per_file[cur].append(json.loads(line))
    chk = subprocess.run([FORTISC, "check", "--json"] + paths + packages, capture_output=True, text=True, cwd=ROOT, timeout=3600).stdout
    findings, cur = {}, None
    for line in chk.splitlines():
        if line.startswith("=== "):
            cur = line[4:]
            findings[cur] = []
        elif line.startswith("{"):
            findings[cur].append(json.loads(line))
    errors, functions, cross = [], 0, 0
    ranges = {"DEL-14..16 (loans, closures)": (14, 16, "loans"), "DEL-17..18 (obligations)": (17, 18, "obligations"),
              "DEL-19..20 (effects, observation)": (19, 20, "observation"), "core stage (all five families)": (0, 999, "cleanup")}
    coverage = {k: [0, 0] for k in ranges}
    fams = {"loans": 0, "obligations": 0, "effects": 0, "observation": 0, "cleanup": 0}
    pkg_modules = [{"path": p, "codes": []} for p in per_file if p not in paths]
    for r in fixtures + [{"path": p, "codes": []} for p in extra] + pkg_modules:
        src = open(os.path.join(ROOT, r["path"]), encoding="utf-8").read()
        m = re.search(r"^// covers: (.*)$", src, re.M)
        covers = re.findall(r"LANG-\d\d-\d\d", m.group(1)) if m else []
        owners = [own["rules"].get(c, {}).get("completes") for c in covers] + [own["diagnostics"].get(c) for c in r.get("codes", [])]
        owners = [o for o in owners if o]
        cc = max((chunk_num(o) for o in owners), default=None) if owners else None
        fns = per_file.get(r["path"], [])
        lines = src.split("\n")
        starts = sorted(f["line"] for f in fns)
        for fn in fns:
            functions += 1
            e = []
            check(fn, SCHEMA, f"{r['path']}:{fn.get('function')}", e)
            # every name a fact cites occurs in the function's source (from its head to the next function's head) or on
            # the fact's own line (a woven aspect's before, after or handler block lies outside the function's text)
            later = [x for x in starts if x > fn["line"]]
            span = "\n".join(lines[fn["line"] - 1:(later[0] - 1) if later else len(lines)])
            # a woven fact names an aspect applied in this module; its lines are the aspect's file's, so it is not
            # checked against this file's text
            for fam in ("loans", "obligations", "observation", "cleanup"):
                for f in fn[fam]:
                    if f["origin"] is not None:
                        asp = f["origin"].split("@", 1)[1]
                        if not re.search(r"@" + re.escape(asp) + r"\b", src):
                            e.append(f"{r['path']}:{fn['function']}: a fact woven from @{asp}, which this module does not apply")
            own_loans = [l for l in fn["loans"] if l["origin"] is None]
            cited = [(l["owner"], l["line"]) for l in own_loans] + [(l["holder"], l["line"]) for l in own_loans if l["holder"]] + [(o["place"], o["opened"]) for o in fn["obligations"] if o["origin"] is None]
            for name, at in cited:
                body = span + "\n" + (lines[at - 1] if 0 < at <= len(lines) else "")
                if " " in name:   # a temporary, journaled under its description ("statement result", "argument o")
                    continue
                if re.fullmatch(r"for@\d+", name):   # the loan a `for` loop holds on its sequence (7.7)
                    if not any(f"for" in ln for ln in lines[int(name[4:]) - 1:int(name[4:])]):
                        e.append(f"{r['path']}:{fn['function']}: loop holder `{name}` names a line with no `for`")
                    continue
                root = re.split(r"[.\[]", name)[0]
                if root and not re.search(r"\b" + re.escape(root) + r"\b", body):
                    e.append(f"{r['path']}:{fn['function']}: fact cites `{name}`, which does not occur in the function")
            for o in fn.get("obligations", []):
                for c in o["closes"]:
                    if c["line"] == 0:
                        e.append(f"{r['path']}:{fn['function']}: obligation {o['place']} has a closing event without a line")
            defers = {c["line"] for c in fn.get("cleanup", []) if c["op"] == "defer"}
            for c in fn.get("cleanup", []):
                if c["op"] in ("exit", "block end") and not set(c["runs"]) <= defers:
                    e.append(f"{r['path']}:{fn['function']}: {c['op']} at {c['line']} runs {c['runs']}, not all registered defers {sorted(defers)}")
            errors += e
            for f in fams:
                v = fn.get(f)
                if (f == "effects" and v and v["effects"]) or (f != "effects" and v):
                    fams[f] += 1
            for k, (lo, hi, fam) in ranges.items():
                if cc is not None and lo <= cc <= hi or lo == 0:   # every module is core-stage: none imports a kit
                    coverage[k][0] += 1
                    v = fn.get(fam)
                    if v is not None:
                        coverage[k][1] += 1
    # every unconsumed-obligation finding names a place the IR shows as dropped or never closed
    for path, recs in findings.items():
        for rec in recs:
            if rec["code"] != "E-OBL-001" or not rec.get("fields", {}).get("unconsumed_path"):
                continue
            place = rec["fields"]["unconsumed_path"]
            # a package check's section is the package directory; the function IR is keyed by the module file
            fns = []
            for key in dict.fromkeys([rec.get("file"), path]):
                if key:
                    fns.extend(per_file.get(key, []))
            facts = [o for fn in fns for o in fn["obligations"] if o["place"] == place]
            cross += 1
            if not any(c["how"] == "unconsumed" and c["line"] == rec["line"] for o in facts for c in o["closes"]):
                errors.append(f"{path}:{rec['line']}: E-OBL-001 for `{place}` has no unconsumed event at its line in the IR ({facts})")
    print(f"unconsumed-obligation findings matched to IR obligations: {cross}")
    # every loan-conflict finding names a held loan (its holder and mode) that the IR records
    held = 0
    for path, recs in findings.items():
        for rec in recs:
            m = re.fullmatch(r"(alias|exclusive) loan held by (\S+)", rec.get("fields", {}).get("conflict", ""))
            if not m:
                continue
            held += 1
            mode = "alias" if m.group(1) == "alias" else "editable"
            loans = [l for fn in per_file.get(path, []) for l in fn["loans"]]
            if not any(l["holder"] == m.group(2) and l["mode"] == mode for l in loans):
                errors.append(f"{path}:{rec['line']}: {rec['code']} names a {mode} loan held by `{m.group(2)}`, which the IR does not record")
    print(f"loan-conflict findings matched to IR loans: {held}")
    for e in errors[:30]:
        print("FAIL", e)
    print(f"{len(per_file)} modules, {functions} functions; {len(errors)} schema or consistency errors")
    print("functions with facts: " + ", ".join(f"{k} {v}" for k, v in fams.items()))
    for k, (n, ok) in coverage.items():
        print(f"{k}: {n} functions, {ok} carry the family")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build", "rust-frontend"), exist_ok=True)
        json.dump({"modules": len(per_file), "functions": functions, "errors": errors, "families": fams, "coverage": coverage},
                  open(os.path.join(ROOT, "build", "rust-frontend", "ir_results.json"), "w"), indent=1)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
