"""Synchronous parity of the three implementations on the G1 subset (DEL-37), and SP-01's closing run.

The three implementations are the definitional interpreter (with the reference checker in front of it), the
development VM and the native lowering (both behind the production frontend). For every fixture of the G1
subset (docs/delivery/subsets.json) that is a whole program over the core library:

- **accept**: the reference checker accepts it; the VM builds and validates its IR; native lowers it to a
  `#![forbid(unsafe_code)]` crate that rustc builds.
- **reject**: the reference checker reports the expected codes; the VM and native refuse it, before lowering,
  with a code the fixture expects.
- **execute**: the interpreter's console lines, final failure or defect marker and record kinds equal the
  VM's and native's.

Application A's G1 part: its tests, run by the interpreter (`fortis test`), the VM and native (`--tests`), give
the same status for every G1 test case. A test case that uses a feature outside G1 is either reported `unbuilt`
by an implementation that does not support the feature yet, or gives the interpreter's status (the VM supports
the codec and async since DEL-38); nothing outside them is unbuilt.

The ownership corpus of SP-01 (its preregistration lists the programs) is reported on its own.

`--core` (DEL-40) takes every core-stage whole program instead of the G1 subset, compares each execute fixture's
trace on all three as well (the interpreter's, the VM's and native code's executor trace), and requires every test
case of application A to agree. A native defect aborts the process (10.8): its output and trace stop at the defect,
which is the permitted consequence when the lines before it are the interpreter's.

  python3 -I conformance/tools/parity_check.py [--core] [--json] [-v]"""
from __future__ import annotations

import hashlib
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
OUT = os.path.join(ROOT, "build", "parity")

from vm_check import features, g1_subset, run_split  # noqa: E402
from native_check import parse as parse_native, native_defect_consequence  # noqa: E402

FAILED_BUILD = ("unlowered", "build-error", "unbuilt", "refused", "missing", "run-error")
KINDS = ("runtime-failure", "defect", "observation-frame", "observation-event", "test-result")


def sha(path):
    return hashlib.sha256(open(os.path.join(ROOT, path), "rb").read()).hexdigest()


def standalone(src):
    return all(m.startswith("fortis.") and m.split(".")[1] in ("prelude", "text", "collections", "numeric", "time", "money", "host")
               for m in re.findall(r"^import\s+([\w.]+)", src, re.M))


def parse_vm(out):
    res, cur = {}, None
    for line in out.splitlines():
        if line.startswith("=== "):
            cur = line[4:]
            res[cur] = {"observed": None, "records": [], "trace": ""}
        elif line.startswith("observed: "):
            res[cur]["observed"] = line[len("observed: "):]
        elif line.startswith("trace: "):
            res[cur]["trace"] = line[len("trace: "):]
        elif line.startswith("record: "):
            res[cur]["records"].append(dict(kv.split("=", 1) for kv in line[len("record: "):].split("; ") if "=" in kv))
    return res


def ownership_corpus():
    text = open(os.path.join(ROOT, "spikes", "SP-01", "README.md"), encoding="utf-8").read()
    progs = re.findall(r"^\| (conformance/programs/[\w]+\.ft) \|", text, re.M)
    lr = sorted(os.path.join("conformance", "corpus", f) for f in os.listdir(os.path.join(ROOT, "conformance", "corpus"))
                if re.match(r"LR-7\.[3-6]-\d+\.ft$", f))
    return progs + lr


def main(argv):
    core = "--core" in argv
    excluded = {"kit", "package"} if core else g1_subset()
    checker = {r["path"]: r for r in json.load(open(os.path.join(ROOT, "conformance", "checker", "results.json"), encoding="utf-8"))["results"]}
    interp = {r["path"]: r for r in json.load(open(os.path.join(ROOT, "conformance", "execution", "results.json"), encoding="utf-8"))["results"]}
    fixtures = []
    for p, r in sorted(checker.items()):
        if r["verdict"] not in ("accept", "reject", "execute") or r["status"] != "PASS" or not p.endswith(".ft"):
            continue
        src = open(os.path.join(ROOT, p), encoding="utf-8").read()
        if set(features(src)) & excluded or not standalone(src):
            continue
        fixtures.append(p)
    vm = parse_vm(run_split("vm", fixtures, timeout=7200))
    native = parse_native(run_split("native", fixtures, timeout=14400))
    rows = []
    for p in fixtures:
        r = checker[p]
        v = vm.get(p, {"observed": "missing", "records": []})
        n = native.get(p, {"observed": "missing", "records": [], "audit": "", "evidence": ""})
        vo = v["observed"] if v["observed"] is not None else "missing"
        no = n["observed"] if n["observed"] is not None else "missing"
        problems = []
        later = r["verdict"] == "reject" and r.get("expected") == "no checker code"
        if later:
            # a reject fixture for a later layer's code (E-EFF-030 is the plan step, X-135): the checker reports nothing,
            # and the VM and the native backend, which run the plan step before anything runs, refuse with that code
            want = set(r["codes"])
            for name, o in (("vm", vo), ("native", no)):
                code = o[len("refused:"):] if o.startswith("refused:") else None
                if code not in want:
                    problems.append(f"{name}: expected the plan step's refusal with one of {sorted(want)}, observed {o!r}")
            ref = f"no checker code; refused by the plan step with {','.join(r['codes'])}"
        elif r["verdict"] == "reject":
            want = set(r["codes"])
            for name, o in (("vm", vo), ("native", no)):
                code = o[len("refused:"):] if o.startswith("refused:") else None
                if code not in want:
                    problems.append(f"{name}: expected a refusal with one of {sorted(want)}, observed {o!r}")
            ref = f"reject:{','.join(r['codes'])}"
        elif r["verdict"] == "accept":
            if vo.startswith(("refused", "unbuilt")):
                problems.append(f"vm: {vo!r}")
            if no.startswith(FAILED_BUILD):
                problems.append(f"native: {no!r}")
            ref = "accept"
        else:
            i = interp.get(p)
            if i is None:
                problems.append("interpreter: no execution result")
                ref = "missing"
            else:
                ref = i["observed"]
                # a native defect aborts the process (10.8): the output may stop at the defect marker
                aborted = no.rpartition("|")[2].startswith("defect:") and (no == ref or native_defect_consequence(i, n, os.path.join(ROOT, p)))
                if vo != ref:
                    problems.append(f"vm output {vo!r}, interpreter {ref!r}")
                if no != ref and not aborted:
                    problems.append(f"native output {no!r}, interpreter {ref!r}")
                if core:
                    it = "|".join(i.get("trace", []))
                    nt = "|".join(next((x.get("events", []) for x in n["records"] if x.get("kind") == "executor-trace"), []))
                    if v.get("trace", "") != it:
                        problems.append(f"vm trace {v.get('trace', '')[:200]!r}, interpreter {it[:200]!r}")
                    if nt != it and not (aborted and it.startswith(nt)):
                        problems.append(f"native trace {nt[:200]!r}, interpreter {it[:200]!r}")
                key = lambda x: (x.get("kind"), str(x.get("code", x.get("class", x.get("status", "")))))
                ik = [key(x) for x in i.get("reports", []) if x.get("kind") in KINDS]
                vk = [key(x) for x in v["records"] if x.get("kind") in KINDS]
                nk = [key(x) for x in n["records"] if x.get("kind") in KINDS]
                if vo == ref and ik != vk:
                    problems.append(f"vm records {vk[:6]}, interpreter {ik[:6]}")
                if no == ref and ik != nk:
                    problems.append(f"native records {nk[:6]}, interpreter {ik[:6]}")
                before = nk[:-1] if nk and nk[-1][0] == "defect" else nk
                if aborted and no != ref and before != ik[:len(before)]:
                    problems.append(f"native records before the abort {nk[:6]}, interpreter {ik[:6]}")
        if (r["verdict"] in ("accept", "execute") or later) and not no.startswith(FAILED_BUILD):
            if "unattributed 0" not in n.get("audit", "") or "boxed closures 0" not in n.get("audit", ""):
                problems.append(f"native audit: {n.get('audit', '')!r}")
            if "rustc=generated_checked" not in n.get("evidence", "") or "unsafe_code=forbidden" not in n.get("evidence", ""):
                problems.append(f"native evidence: {n.get('evidence', '')!r}")
        rows.append({"path": p, "sha256": sha(p), "verdict": r["verdict"], "reference": ref, "vm": vo, "native": no,
                     "agree": not problems, "problems": problems})
    # application A's G1 part
    app = application_a(excluded, core)
    # SP-01's ownership corpus
    own = ownership_corpus()
    by = {x["path"]: x for x in rows}
    own_rows = []
    for p in own:
        x = by.get(p)
        own_rows.append({"path": p, "sha256": sha(p), "in_g1": x is not None, "verdict": x["verdict"] if x else checker.get(p, {}).get("verdict"),
                         "agree": x["agree"] if x else None, "why_not": None if x else (checker.get(p, {}).get("detail") or checker.get(p, {}).get("observed") or "not a whole program")})
    for x in rows:
        if not x["agree"]:
            print(f"DISAGREE {x['verdict']} {x['path']}")
            for pr in x["problems"]:
                print(f"      {pr[:400]}")
    for c in app["cases"]:
        if not c["agree"]:
            print(f"DISAGREE application A {c['test']} {c['case']}: {c}")
    counts = {k: [sum(1 for x in rows if x["verdict"] == k), sum(1 for x in rows if x["verdict"] == k and x["agree"])] for k in ("accept", "reject", "execute")}
    label = "core-stage" if core else "G1"
    print(f"{label} fixtures on all three: {len(rows)}, {sum(x['agree'] for x in rows)} agree, {sum(not x['agree'] for x in rows)} disagree")
    for k, (n, a) in counts.items():
        print(f"  {k}: {n}, {a} agree")
    if core:
        print(f"application A: {app['g1_cases']} test cases, {app['agree']} agree on the interpreter, the VM and native; unbuilt: {app['unbuilt'] or 'none'}")
    else:
        print(f"application A: {app['g1_cases']} G1 test cases, {app['agree']} agree on the interpreter, the VM and native; outside G1 ({', '.join(app['outside_g1'])}): VM unbuilt {app['unbuilt_vm'] or 'none'}, native unbuilt {app['unbuilt_native'] or 'none'}, every built one agrees: {all(b['ok'] for b in app['outside_cases'])}")
    oi = [x for x in own_rows if x["in_g1"]]
    print(f"SP-01 ownership corpus: {len(own_rows)} programs, {len(oi)} whole G1 programs, {sum(1 for x in oi if x['agree'])} agree on all three; {len(own_rows) - len(oi)} are fragments judged by the checker only")
    ok = all(x["agree"] for x in rows) and app["ok"] and all(x["agree"] for x in oi)
    os.makedirs(OUT, exist_ok=True)
    if "--json" in argv:
        json.dump({"rows": rows, "application_a": app, "ownership_corpus": own_rows, "counts": counts}, open(os.path.join(OUT, "report-core.json" if core else "report.json"), "w", encoding="utf-8"), indent=1)
    print("parity check: " + ("OK" if ok else "FAILED"))
    return 0 if ok else 1


def application_a(excluded, core=False):
    d = os.path.join("acceptance", "a")
    it = subprocess.run([os.path.join(ROOT, "bin", "fortis"), "test", d, "--format", "json"], capture_output=True, text=True, cwd=ROOT, timeout=3600)
    ref = {}
    for line in it.stderr.splitlines():
        if line.strip().startswith("{"):
            r = json.loads(line)
            if r.get("kind") == "test-result":
                ref[(r["test"], r.get("case", ""))] = r["status"]
    vm = parse_vm(subprocess.run([FORTISC, "vm", d, "--tests"], capture_output=True, text=True, cwd=ROOT, timeout=3600).stdout).get(d, {"records": []})
    nat = parse_native(subprocess.run([FORTISC, "native", d, "--tests"], capture_output=True, text=True, cwd=ROOT, timeout=3600).stdout).get(d, {"records": []})
    vs = {(r["test"], r.get("case", "")): r["status"] for r in vm["records"] if r.get("kind") == "test-result"}
    ns = {(r["test"], r.get("case", "")): r["status"] for r in nat["records"] if r.get("kind") == "test-result"}
    # the tests outside G1: their bodies use an excluded feature
    outside = set()
    for f in sorted(os.listdir(os.path.join(ROOT, d, "src"))):
        src = open(os.path.join(ROOT, d, "src", f), encoding="utf-8").read()
        for m in re.finditer(r'^(?:test|property) "([^"]+)"[^\n]*\{\n(.*?)^\}', src, re.M | re.S):
            if set(features(m.group(2))) & excluded and not core:
                outside.add(m.group(1))
    cases = []
    for (t, c), s in sorted(ref.items()):
        if t in outside:
            continue
        cases.append({"test": t, "case": c, "interpreter": s, "vm": vs.get((t, c)), "native": ns.get((t, c)), "agree": s == vs.get((t, c)) == ns.get((t, c))})
    unbuilt_vm = sorted({t for (t, _), s in vs.items() if s == "unbuilt"})
    unbuilt_nat = sorted({t for (t, _), s in ns.items() if s == "unbuilt"})
    beyond = []
    for (t, c), s in sorted(ref.items()):
        if t not in outside:
            continue
        for impl, got in (("vm", vs.get((t, c))), ("native", ns.get((t, c)))):
            beyond.append({"test": t, "case": c, "implementation": impl, "interpreter": s, "observed": got, "ok": got in (s, "unbuilt")})
    ok = all(c["agree"] for c in cases) and set(unbuilt_vm) <= outside and set(unbuilt_nat) <= outside and all(b["ok"] for b in beyond) and len(cases) > 0
    return {"cases": cases, "g1_cases": len(cases), "agree": sum(c["agree"] for c in cases), "unbuilt": sorted(set(unbuilt_vm) | set(unbuilt_nat)),
            "unbuilt_vm": unbuilt_vm, "unbuilt_native": unbuilt_nat, "outside_g1": sorted(outside), "outside_cases": beyond, "ok": ok}


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
