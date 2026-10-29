"""Native runner for the experimental lowering (DEL-25): lower a fixture, build it with cargo (offline, no
dependencies beyond the audited runtime), run it when it has a `main`, and report what the harness compares.

  python3 -I compiler/experimental-lowering/native.py FILE.ft            # lower, build, run one program
  python3 -I compiler/experimental-lowering/native.py --corpus            # the DEL-25 corpora and the synchronous execute fixtures

Crates are written under build/native/<fixture>/ with one shared target directory."""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
sys.path.insert(0, ROOT)
BUILD = os.path.join(ROOT, "build", "native")
TARGET = os.path.join(BUILD, "target")

from lower import LowerError, lower_file  # noqa: E402

EXCLUDED: list = []     # corpus programs outside the synchronous slice (listed in the decision records)
# Core-library pieces the experimental runtime does not provide (Map, Set, Array, atomics, Mutex, Failures, the
# numeric and collections functions): their native lowering is DEL-36's production lowering.
LIBRARY_LATER = ("Map[", "Set[", "Array[", "AtomicInt", "AtomicBool", "Mutex", "Failures.", "fortis.numeric", "fortis.collections",
                 ".toText(", ".fromText(", ".toBytes(", ".fromBytes(")          # the native canonical codec arrives with the library lowering (DEL-36)
EXCLUDED_WORDS = LIBRARY_LATER + ("async ", "await", "scope ", "defer", "\n@", "capture ", "record ", "\ntest ", "\nproperty ", "launch", "injectDefect", "Decimal", "Duration", "Float", "checkpoint", "aspect ", "observation ", "dynamic ", "Scalar", "Builder", "contract ", "implement ", "interface ", "mixin ", "derive ", "Weak", "implements ", "...", "fortis.data", "fortis.http", "fortis.security")
RECORD_LIT = re.compile(r"[=(,]\s*\{\s*\w+:|\b\d+(\.\d+)?d\b|\b\d+(ms|s|m|h|ns|us)\b")   # record literals, Decimal and Duration literals


def code_only(src: str) -> str:
    return "\n".join(l.split("//")[0] for l in src.split("\n"))


def corpus() -> list:
    """Fixture paths: the accepted programs of the SP-01..05 corpora plus the synchronous execute fixtures."""
    out = []
    for s in ("SP-01", "SP-02", "SP-03", "SP-04", "SP-05"):
        p = os.path.join(ROOT, "spikes", s, "README.md")
        if not os.path.exists(p):
            print(f"native: skipped the accepted programs of {s}'s corpus: its spike record is a maintainers' planning "
                  f"file and is not in this checkout", file=sys.stderr)
            continue
        for l in open(p, encoding="utf-8"):
            m = re.match(r"\| (conformance/programs/\S+) \|", l)
            if m and m.group(1) not in out:
                src = code_only(open(os.path.join(ROOT, m.group(1)), encoding="utf-8").read())
                if any(w in src for w in ("await", "async ", "capture ", "record ", "scope ")):
                    EXCLUDED.append(m.group(1))            # the async and observation parts of the corpora are not in the sync slice
                    continue
                out.append(m.group(1))
    pd = os.path.join(ROOT, "conformance", "programs")
    for n in sorted(os.listdir(pd)):
        if not n.startswith("exec_") or not n.endswith(".ft"):
            continue
        src = open(os.path.join(pd, n), encoding="utf-8").read()
        if "// requires: runtime" not in src:
            continue
        if any(w in code_only(src) for w in EXCLUDED_WORDS) or RECORD_LIT.search(code_only(src)):
            continue
        if n.startswith("exec_host_"):
            EXCLUDED.append(f"conformance/programs/{n}")   # the native host adapters are DEL-47's
            continue
        if n == "exec_defect_stack.ft":
            EXCLUDED.append(f"conformance/programs/{n}")   # an optimised build turns the unbounded recursion into a loop (X-106)
            continue
        out.append(f"conformance/programs/{n}")
    return out


def lower_and_build(rel: str) -> dict:
    """{status: lowered|refused|lower-error|build-error|ok, ...}"""
    path = os.path.join(ROOT, rel)
    fid = os.path.basename(rel)[:-3]
    if fid[:1].isdigit():
        fid = "f_" + fid                                   # a crate name cannot start with a digit
    out_dir = os.path.join(BUILD, fid)
    try:
        lw = lower_file(path, out_dir)
    except LowerError as e:
        if "the checker rejected" in str(e):
            return {"id": fid, "status": "refused", "detail": str(e)}
        return {"id": fid, "status": "lower-error", "detail": str(e)}
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_NET_OFFLINE="true")
    r = subprocess.run(["cargo", "build", "-q", "--offline", "--release"], cwd=out_dir, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        errs = [l for l in r.stderr.splitlines() if l.startswith("error")]
        return {"id": fid, "status": "build-error", "detail": "\n".join(errs[:6]) or r.stderr[-800:], "copies": lw.copies}
    return {"id": fid, "status": "ok", "copies": lw.copies, "has_main": lw.has_main, "crate": fid}


def run(fid: str) -> dict:
    exe = os.path.join(TARGET, "release", fid)
    try:
        r = subprocess.run([exe], capture_output=True, text=True, timeout=30)
    except subprocess.TimeoutExpired:
        return {"stdout": [], "exit": -999, "records": [], "timeout": True}
    records = []
    for l in r.stderr.splitlines():
        try:
            records.append(json.loads(l))
        except ValueError:
            pass
    return {"stdout": r.stdout.rstrip("\n").split("\n") if r.stdout else [], "exit": r.returncode, "records": records}


def observe(rel: str) -> dict:
    """What the differential harness compares: verdict, output (stdout lines joined by |), trace."""
    b = lower_and_build(rel)
    if b["status"] != "ok":
        return {"verdict": "reject" if b["status"] == "refused" else "native-error", "detail": b.get("detail"), "output": None, "trace": None, "copies": b.get("copies", [])}
    obs = {"verdict": "accept", "output": None, "trace": None, "copies": b["copies"]}
    if b["has_main"]:
        r = run(b["crate"])
        fails = [x["code"] for x in r["records"] if x.get("kind") == "runtime-failure" and x.get("reported_by") == "main"]
        reports = [x["code"] for x in r["records"] if x.get("kind") == "runtime-failure"]
        obs["output"] = "|".join(r["stdout"])
        exit_ = "ok" if r["exit"] == 0 else (f"fail:{fails[-1]}" if fails else f"defect:{r['exit']}")
        obs["trace"] = {"reports": reports, "exit": exit_}
    return obs


def main(argv):
    if argv and argv[0] == "--corpus":
        results = {}
        for rel in corpus():
            o = observe(rel)
            results[rel] = o
            print(f"{o['verdict']:<13} {rel}" + (f"  {o['detail'][:200]}" if o.get("detail") else "") + (f"  -> {o['output']}" if o.get("output") is not None else ""))
        ok = sum(1 for o in results.values() if o["verdict"] == "accept")
        print(f"\n{len(results)} programs, {ok} lowered and built, {len(results) - ok} not")
        json.dump(results, open(os.path.join(BUILD, "results.json"), "w"), indent=1)
        json.dump(EXCLUDED, open(os.path.join(BUILD, "excluded.json"), "w"), indent=1)
        print("outside the synchronous slice:", ", ".join(os.path.basename(e) for e in EXCLUDED))
        return 0 if ok == len(results) else 1
    o = observe(argv[0])
    print(json.dumps(o, indent=1))
    return 0


if __name__ == "__main__":
    os.makedirs(BUILD, exist_ok=True)
    sys.exit(main(sys.argv[1:]))
