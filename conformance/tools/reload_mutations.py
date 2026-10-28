#!/usr/bin/env python3
"""SP-07's apparatus check (spikes/SP-07/design.md 13): run the harness against each injected fault.

    python3 conformance/tools/reload_mutations.py --json OUT --fault-fortisc PATH [harness options]

The fault build (fortisc with `--features reload-faults`) reads the fault from FORTIS_RELOAD_FAULT. Each fault names
the family meant to catch it. A fault is killed when a check of that family fails. A control run on the fault build
with no fault must pass every check. A mutated run that writes no results, or results not marked complete, is an
apparatus failure.
"""
import argparse
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))

FAULTS = [
    ("call-oldest", "a new invocation runs the oldest live generation", "I"),
    ("launch-current", "a launched task runs the session's current generation", "I"),
    ("index-not-identity", "a code value from another generation runs by its creating generation's index", "I"),
    ("future-no-move", "a future is rebound without moving its arguments", "I"),
    ("consts-shared", "constants are cached across generations", "I"),
    ("print-at-collect", "a print is attributed when the driver collects it", "I"),
    ("rebuild-all", "a reload rebuilds every scope's state", "M"),
    ("rebuild-shop-silently", "a compatible reload silently rebuilds the shop's state", "M"),
    ("body-as-summary", "`body` changes are reported as `summary`", "C"),
    ("env-ignored", "environment changes are ignored", "C"),
    ("lambda-ordinal", "lambdas are matched by ordinal only", "C"),
    ("layout-unordered", "layout fingerprints ignore field order", "C"),
    ("dispatch-ignored", "method changes of a type are ignored", "C"),
    ("di-body-compatible", "DI body edits are compatible", "C"),
    ("r1-no-payloads", "R1 ignores variant payloads", "R"),
    ("skip-r2", "R2 is skipped", "R"),
    ("restart-no-cancel", "restart does not cancel in-flight invocations", "R"),
    ("release-before-drain", "restart releases the state before the drain", "R"),
    ("release-new", "the old state is released on the new generation", "R"),
    ("path-first", "an explanation's path is cut to its first step", "X"),
    ("task-keeps-gen", "task records keep their generation after completion", "G"),
    ("session-holds-strong", "the session holds old generations strongly", "G"),
    ("cap-after-install", "the generation cap is checked after installing", "G"),
    ("rejected-resets", "a rejected reload restarts every scope", "J"),
    ("unseeded-order", "restarts are ordered by an unseeded hash", "D"),
    ("nested-lambda-flat", "a lambda's own lambdas are left out of its fingerprint", "I"),
    ("r2-shallow", "R2 follows only fields and type arguments to a Mutex", "R"),
    ("const-shallow", "Dep stops at a constant, not following its initializer", "R"),
    ("generic-by-use", "genericity is read from surviving type-parameter uses only", "C"),
    ("consts-kept", "a reclaimed generation's constant cache entries are kept", "G"),
    ("zero-bytes", "the memory cap's accounting counts no bytes", "G"),
    ("launch-current-nap", "a join's children run the session's current generation", "N"),
    ("r2-no-subst", "Rep walks a generic type's fields without its type arguments", "R"),
    ("detached-keeps-result", "a detached task's result stays in its task record", "R"),
    ("frames-off", "the frame log records nothing", "N"),
    ("future-args-kept", "an unawaited future's owned arguments are not destroyed with it", "R"),
    ("recipe-unrefined", "a restart recipe's arguments are checked without their refinements", "R"),
    ("path-rewire", "a dependency explanation is routed through real but unrelated calls", "X"),
    ("held-uncounted", "a stalled scope's kept state is left out of the caps", "G"),
    ("reader-stall-admitted", "a reload is installed while its old readers have not drained", "G"),
]


def family_of(check):
    f = check["family"]
    return {"I1": "I", "I2": "I", "I3": "I", "I4": "I", "I5": "I", "R-a": "R", "R-b": "R", "R-c": "R", "G1": "G", "G2": "G", "G3": "G", "G4": "G",
            "N1": "N", "N2": "N", "N3": "N", "N4": "N", "N5": "N"}.get(f, f)


def run(args, fault, out):
    """One harness run; its results only if this run wrote them, for these parameters (no stale or foreign file)."""
    if os.path.exists(out):
        os.remove(out)
    # every run has its own logs directory, so the control never overwrites the registered run's (build/reload)
    work = os.path.join(ROOT, "build", "reload-mutations", fault or "control")
    cmd = [sys.executable, os.path.join(HERE, "reload_check.py"), "--json", out, "--fortisc", args.fault_fortisc, "--seed", str(args.seed), "--soak", str(args.soak), "--work", work, "--fault-fortisc", args.fault_fortisc]
    if fault:
        cmd += ["--fault", fault]
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=3600)
    tail = (p.stdout + p.stderr)[-800:]
    if p.returncode not in (0, 1):
        return None, f"the harness exited {p.returncode}: {tail}"
    try:
        with open(out, encoding="utf-8") as f:
            res = json.load(f)
    except (OSError, ValueError):
        return None, tail
    if res.get("fault") != fault or res.get("seed") != args.seed or res.get("soak") != args.soak:
        return None, f"the results are not this run's: {res.get('fault')} {res.get('seed')} {res.get('soak')}"
    return res, tail


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", required=True)
    ap.add_argument("--fault-fortisc", required=True)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--soak", type=int, default=1000)
    a = ap.parse_args()
    t0 = time.time()
    tmp = os.path.join(ROOT, "build", "reload-mutations")
    os.makedirs(tmp, exist_ok=True)
    records = []
    control, tail = run(a, None, os.path.join(tmp, "control.json"))
    control_ok = bool(control) and control["complete"] and control["passed"] == control["total"]
    print(f"control (fault build, no fault): {'pass' if control_ok else 'FAIL'} {control and control['passed']}/{control and control['total']}", flush=True)
    for name, about, fam in FAULTS:
        res, tail = run(a, name, os.path.join(tmp, f"{name}.json"))
        if res is None or not res.get("complete"):
            rec = {"fault": name, "about": about, "family": fam, "status": "apparatus-failure", "detail": tail if res is None else "results not marked complete: " + "; ".join(res.get("exceptions", []))[:600]}
        else:
            failed = [c for c in res["checks"] if not c["pass"]]
            mine = [c["name"] for c in failed if family_of(c) == fam]
            rec = {"fault": name, "about": about, "family": fam, "status": "killed" if mine else "survived", "by": mine[:8], "other_failures": sorted({c["name"] for c in failed if family_of(c) != fam})[:8]}
        records.append(rec)
        print(f"{rec['status']:<17} {name} ({fam}): {rec.get('by') or rec.get('detail', '')}", flush=True)
        # the records so far, so an interrupted run still shows what it found
        with open(a.json + ".partial", "w", encoding="utf-8") as f:
            json.dump(records, f, indent=1, ensure_ascii=False)
    killed = sum(r["status"] == "killed" for r in records)
    failures = sum(r["status"] == "apparatus-failure" for r in records)
    out = {"schema": "fortis.reload-mutations/v1", "seed": a.seed, "soak": a.soak, "control": {"complete": bool(control and control["complete"]), "passed": control and control["passed"], "total": control and control["total"], "pass": control_ok},
           "faults": len(FAULTS), "killed": killed, "apparatus_failures": failures, "seconds": round(time.time() - t0, 1), "records": records}
    with open(a.json, "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1, ensure_ascii=False)
        f.write("\n")
    print(f"reload mutations: {killed}/{len(FAULTS)} killed, {failures} apparatus failures, control {'pass' if control_ok else 'FAIL'}, {out['seconds']} s")
    return 0 if killed == len(FAULTS) and control_ok else 1


if __name__ == "__main__":
    sys.exit(main())
