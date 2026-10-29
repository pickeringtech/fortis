"""The production native lowering against the definitional interpreter (DEL-36).

1. Every G1-subset execute fixture is lowered (fortisc native: typed IR, validated, lowered by fortis-lower to a
   `#![forbid(unsafe_code)]` crate over fortis-native), built offline and run; its console lines and final
   `fail:`/`defect:` marker, and the kinds and codes of its records, are compared with the interpreter's
   (conformance/execution/results.json).
2. Every G1-subset accepted program (execute fixtures and `// accept` programs) lowers and builds; the copy and
   reference-count audit attributes every clone to a source `copy` or `retain`; no closure is boxed.

  python3 -I conformance/tools/native_check.py [--json] [-v] [--only SUBSTRING]"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
sys.path.insert(0, HERE)
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")

from vm_check import features, g1_subset, run_split  # noqa: E402


def parse(out):
    res, cur = {}, None
    for line in out.splitlines():
        if line.startswith("=== "):
            cur = line[4:]
            res[cur] = {"observed": None, "records": [], "audit": ""}
        elif line.startswith("observed: "):
            res[cur]["observed"] = line[len("observed: "):]
        elif line.startswith("audit: "):
            res[cur]["audit"] = line[len("audit: "):]
        elif line.startswith("evidence: "):
            res[cur]["evidence"] = line[len("evidence: "):]
        elif line.startswith("record: "):
            try:
                res[cur]["records"].append(json.loads(line[len("record: "):]))
            except json.JSONDecodeError:
                pass
    return res


def native_defect_consequence(r, n, path) -> bool:
    """A defect aborts the process natively and terminates only the task on the VM and the interpreter (Language
    Reference 10.8). A native run agrees when its lines before the defect marker are the interpreter's lines up to
    that point and the class is the one the program raises (the interpreter's marker or an injected class)."""
    no = n["observed"] or ""
    if "defect:" not in no:
        return False
    head, _, cls = no.rpartition("defect:")
    lines = [l for l in head.split("|") if l]
    ref = [l for l in (r["observed"] or "").split("|") if l and not l.startswith("defect:")]
    if lines != ref[:len(lines)]:
        return False
    src = open(path, encoding="utf-8").read()
    # the interpreter terminated a task with a defect (a supervisor's TaskCompletion.Defect, 10.8)
    task_defect = any(x.get("kind") == "task-completion" and x.get("outcome") == "Defect" for x in r.get("reports", []))
    return f"defect:{cls}" in (r["observed"] or "") or f'injectDefect("{cls}")' in src or task_defect


def src_of(p):
    return open(os.path.join(ROOT, p), encoding="utf-8").read()


def main(argv):
    only = argv[argv.index("--only") + 1] if "--only" in argv else None
    interp = {r["path"]: r for r in json.load(open(os.path.join(ROOT, "conformance", "execution", "results.json"), encoding="utf-8"))["results"]}
    # `--core`: every core-stage whole program (DEL-39), not only the G1 subset
    core = "--core" in argv
    excluded = {"kit", "package"} if core else g1_subset("native")
    results = json.load(open(os.path.join(ROOT, "conformance", "results.json"), encoding="utf-8"))["results"]
    accept = sorted({r["path"] for r in results if r["path"].endswith(".ft") and r["verdict"] == "accept" and r.get("status") in ("PASS", "INFO") and "statements" not in (r.get("detail") or "")})
    execs = sorted(interp)
    if only:
        execs = [p for p in execs if only in p]
        accept = [p for p in accept if only in p]

    def g1(p):
        src = open(os.path.join(ROOT, p), encoding="utf-8").read()
        return not (set(features(src)) & excluded), src

    def standalone(p):
        """A program whose imports are all core-library modules (a fragment importing an absent package cannot be built)."""
        src = open(os.path.join(ROOT, p), encoding="utf-8").read()
        return all(m.startswith("fortis.") and m.split(".")[1] in ("prelude", "text", "collections", "numeric", "time", "money", "host") for m in re.findall(r"^import\s+([\w.]+)", src, re.M))
    rows = []
    g1_exec = []
    for p in execs:
        ok, src = g1(p)
        if ok:
            g1_exec.append(p)
    out = parse(run_split("native", g1_exec, timeout=7200))
    kinds = ("runtime-failure", "defect", "observation-frame", "observation-event", "test-result", "buffer-overflow")
    for p in g1_exec:
        r = interp[p]
        n = out.get(p, {"observed": "missing", "records": []})
        problems = []
        if n["observed"] != r["observed"] and not native_defect_consequence(r, n, os.path.join(ROOT, p)):
            problems.append(f"output: interpreter {r['observed']!r}, native {n['observed']!r}")
        key = lambda x: (x.get("kind"), str(x.get("code", x.get("class", x.get("status", "")))))
        ir = [key(x) for x in r.get("reports", []) if x.get("kind") in kinds and x.get("kind") != "buffer-overflow"]
        nr = [key(x) for x in n["records"] if x.get("kind") in kinds and x.get("kind") != "buffer-overflow"]
        if ir != nr and n["observed"] == r["observed"]:
            problems.append(f"records: interpreter {ir[:6]}, native {nr[:6]}")
        # the executor's schedule: a fixture that fixes one (`execute-trace`) gets the same trace natively (12.3)
        want = re.search(r"^//\s*execute-trace:\s*(.*)$", src_of(p), re.M)
        if want and not problems:
            got = "|".join(next((x.get("events", []) for x in n["records"] if x.get("kind") == "executor-trace"), []))
            expected = want.group(1).strip()
            # a native defect aborts the process (10.8): the trace stops where the defect is raised
            aborted = (n["observed"] or "").rpartition("|")[2].startswith("defect:") and expected.startswith(got)
            if got != expected and not aborted:
                problems.append(f"trace: expected {expected!r}, native {got!r}")
        if n["observed"] == r["observed"] and "rustc=generated_checked" not in n.get("evidence", ""):
            problems.append(f"evidence: rustc acceptance not recorded as generated_checked ({n.get('evidence', '')!r})")
        rows.append({"path": p, "kind": "execute", "agree": not problems, "problems": problems, "audit": n.get("audit", ""), "evidence": n.get("evidence", "")})
    # library differential (X-119): programs using a library function with a hand-written native body are built
    # again with the bodies lowered from library/fortis, and both builds must print the same
    uses = re.compile(r"\.(map|filter|countWhere|reversed|concat)\(|\bsorted\(")
    diff = [p for p in g1_exec if uses.search(open(os.path.join(ROOT, p), encoding="utf-8").read())]
    lowered = parse(run_split("native", diff, env={**os.environ, "FORTIS_NATIVE_LIBRARY_BODIES": "0"}, timeout=7200))
    for p in diff:
        a, b = out.get(p, {}).get("observed"), lowered.get(p, {}).get("observed")
        rows.append({"path": p, "kind": "library", "agree": a == b and a is not None, "problems": [] if a == b else [f"hand-written bodies {a!r}, lowered bodies {b!r}"], "audit": lowered.get(p, {}).get("audit", "")})
    # accepted programs of the G1 subset lower and build
    g1_accept = [p for p in accept if p not in interp and g1(p)[0] and standalone(p)]
    acc = parse(run_split("native", g1_accept, timeout=7200))
    for p in g1_accept:
        o = acc.get(p, {}).get("observed") or ""
        bad = o.startswith(("unlowered", "build-error", "unbuilt", "refused", "missing", "run-error"))
        ev = acc.get(p, {}).get("evidence", "")
        problems = [o] if bad else ([] if "rustc=generated_checked" in ev and "proved" not in ev else [f"evidence: {ev!r}"])
        rows.append({"path": p, "kind": "accept", "agree": not problems, "problems": problems, "audit": acc.get(p, {}).get("audit", ""), "evidence": ev})
    for x in rows:
        if not x["agree"] and ("-v" in argv or True):
            print(f"DISAGREE {x['kind']} {x['path']}")
            for pr in x["problems"]:
                print(f"      {pr[:400]}")
    ex = [x for x in rows if x["kind"] == "execute"]
    ac = [x for x in rows if x["kind"] == "accept"]
    label = "core-stage" if core else "G1"
    print(f"\n{label} execute fixtures: {len(ex)}, {sum(x['agree'] for x in ex)} agree with the interpreter")
    print(f"{label} accepted programs: {len(ac)}, {sum(x['agree'] for x in ac)} lower and build")
    built = [x for x in rows if x["kind"] in ("execute", "accept")]
    print(f"evidence: {sum('rustc=generated_checked' in x.get('evidence', '') for x in built)} of {len(built)} builds record rustc acceptance as generated_checked, {sum('proved' in x.get('evidence', '') for x in built)} claim proved")
    lb = [x for x in rows if x["kind"] == "library"]
    print(f"library differential: {len(lb)} programs, {sum(x['agree'] for x in lb)} print the same with hand-written and lowered library bodies")
    unattributed = sum(1 for x in rows if "unattributed 0" not in x["audit"] and x["audit"])
    boxed = sum(1 for x in rows if x["audit"] and "boxed closures 0" not in x["audit"])
    print(f"audit: {unattributed} programs with unattributed clones, {boxed} with boxed closures")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build", "native-prod"), exist_ok=True)
        json.dump({"rows": rows}, open(os.path.join(ROOT, "build", "native-prod", "results-core.json" if core else "results.json"), "w"), indent=1)
    return 0 if all(x["agree"] for x in rows) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
