#!/usr/bin/env python3
"""The gate: every check of the repository, in order, on one working tree, each under its own deadline, with one
verdict.
    python3 -I conformance/tools/gate.py [--tier quick|full] [--jobs N|auto] [--thorough] [--json OUT]
                                         [--tree DIR] [--from STEP] [--only STEP,...]
With --tier quick --changed BASE, ci_plan.py selects the union of checks affected by
the merge-base diff and prints why. Without --changed, the tier runs as before.
The unit step runs first and stops the gate on failure. After it passes, every selected step runs, and the
report lists each step's verdict, time and the tail of its output.

Two tiers. `quick` is what a pull request must pass, locally before it is marked ready and then in CI (ci.yml):
every behavioural step (programs run and compared on the interpreter, the VM and natively; every documentation
example on both checkers, the interpreter and the VM; both acceptance applications on every backend; the pilot's
tests), less the compile-side and records-side steps in QUICK_SKIPS, each with its reason, and with the few
lighter or split forms in QUICK_FORMS. `full`, the default, runs every step; the release workflow (release.yml)
runs it at every `v*` tag with `--thorough`, which gives some steps larger budgets (THOROUGH). A release candidate
and a release need a full pass. The gate prints its tier first and every step it left out last, and the report
records `tier`, `skipped`, `reduced` and `thorough`: a quick pass never reads as a full one.

`--jobs N` runs up to N steps at once where they share no writable state (PHASE_OF; `auto`, the default, is a
quarter of the machine's cores, at most 4, and 1 in a CI shard). Units run first and alone, then the builds, then the syntax
run whose results most checks read, then the independent checks together, then the checks that read the
checker's or the interpreter's results, then the steps that must run alone, then the plan, which reads every
result. Every concurrent step writes only its own report (build/<step>.json or conformance/<step>/results.json)
and its own build/ subdirectory; the steps that build programs natively share build/native-build and
build/native-prod, where a program's crate is named after its file, so they run one after another in a lane of
their own (NATIVE_LANE). The report keeps the step order. `--jobs 1` runs every step in order, one at a time.

`--shard I/N` runs the I-th of N shards of a tier, for CI, where the shards run on separate machines at once
(`--combine REPORT...` then checks that the shards' reports cover the tier and all passed). The build steps are
left to CI's own build jobs. The steps are dealt out longest first, by the durations in gate-timings.json (CI's
own, refreshed with `--timings-from REPORT...`; a step the table lacks counts DEFAULT_SECONDS), to the shard with
the least work so far; a step longer than half a shard's share is split where it can be (SPLITS: docset by page,
the pilot's tests by module, parity by fixture). Each shard first runs the steps whose results its own steps read (syntax always,
and PREREQS), so a shard never reads results another machine produced. With `--toolchain CMD`, a shard starts
before CI's build job ends: it runs the steps that need no compiled toolchain first (its prerequisites, mostly),
then CMD, which fetches the build's binaries, then the rest.

Every step runs with its own empty temporary directory (TMPDIR, under build/tmp/ in a directory of the run's own),
removed when the step ends, so no step sees another's temporary files, nor another gate's on the same machine.

A step that checks the maintainers' planning files (the known-limitations pages against the defect register, and
the delivery plan itself) runs only when its tool is in the checkout. Without it the step is reported as skipped,
with the reason, and does not count against the verdict. Exit 0 only if every step that ran passed.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time

# (name, command, deadline in minutes). The order is the runner order of the delivery plan: the reference toolchain,
# then the Rust frontend and its harnesses, the backends, the tooling, acceptance, and the plan itself.
STEPS = [
    ("unit", "python3 -I conformance/tools/unit_check.py --json", 1),
    ("hygiene", "python3 -I conformance/tools/hygiene_check.py --json", 10),
    ("cargo-build-release", "cd compiler && cargo build --release && cargo build --release -p fortisc --features qdb-faults --target-dir target/faults", 60),
    ("cargo-build-debug", "cd compiler && cargo build", 60),
    # the allocator test (query-db's tests/alloc_run.rs) on its own, with one harness thread: cargo test leaves it out
    ("cargo-test", "cd compiler && cargo test --release && cargo test --release -p fortis-query-db --test alloc -- --test-threads=1", 60),
    ("syntax", "python3 -I conformance/tools/run.py --json", 30),
    ("names", "python3 -I conformance/tools/names_check.py", 30),
    ("checker", "python3 -I conformance/tools/checker_check.py", 60),
    ("packages", "python3 -I conformance/tools/packages_check.py", 30),
    ("execute", "python3 -I conformance/tools/execute_check.py", 60),
    ("cfg", "python3 -I conformance/tools/cfg_check.py", 30),
    ("aspects", "python3 -I conformance/tools/aspects_check.py", 30),
    ("diagnostics", "python3 -I conformance/tools/diagnostics_check.py", 30),
    ("genparser", "python3 -I conformance/tools/genparser_check.py", 30),
    ("library", "python3 -I conformance/tools/library_check.py --json", 30),
    ("codec", "python3 -I conformance/tools/codec_check.py", 30),
    ("frontend", "python3 -I conformance/tools/frontend_check.py --binary compiler/target/debug/fortisc", 90),
    ("ir", "python3 -I conformance/tools/ir_check.py --json", 60),
    ("corrections", "python3 -I conformance/tools/corrections_check.py --json", 30),
    ("vm", "python3 -I conformance/tools/vm_check.py --json", 60),
    ("validator", "python3 -I conformance/tools/validator_check.py --json", 60),
    ("native", "python3 -I conformance/tools/native_check.py --json", 120),
    ("parity", "python3 -I conformance/tools/parity_check.py --core --json", 120),
    ("shapes", "python3 -I conformance/tools/shapes_check.py --json", 30),
    ("adapters", "python3 -I conformance/tools/adapter_check.py --json", 60),
    ("di", "python3 -I conformance/tools/di_check.py --json", 60),
    ("querydb", "python3 -I conformance/tools/querydb_check.py --json", 60),
    ("fmt", "python3 -I conformance/tools/fmt_check.py --json", 30),
    ("view", "python3 -I conformance/tools/view_check.py --json", 30),
    ("lock", "python3 -I conformance/tools/lock_check.py --json", 30),
    ("batch", "python3 -I conformance/tools/batch_check.py --json", 30),
    ("tests", "python3 -I conformance/tools/tests_check.py --json", 60),
    ("docs", "python3 -I conformance/tools/docs_check.py --json", 30),
    ("website", "python3 -I website/build.py --check", 30),
    ("docset", "python3 -I conformance/tools/docset_check.py --json", 90),
    ("showcase", "python3 -I conformance/tools/showcase_check.py --json", 15),
    ("cite", "python3 -I conformance/tools/cite_check.py --json", 15),
    ("site", "python3 -I conformance/tools/site.py --json", 30),
    ("terms", "python3 -I conformance/tools/terms_check.py --json", 15),
    ("libref", "python3 -I conformance/tools/libref.py --check", 10),
    ("limitations", "python3 -I docs/delivery/tools/limitations.py --check", 5),
    ("pilot", "bin/fortis test pilot/ledger --format json && compiler/target/release/fortisc test --format json pilot/ledger", 30),
    ("api", "python3 -I conformance/tools/api_check.py --json", 30),
    ("limits", "python3 -I conformance/tools/limits_check.py --json", 30),
    ("features", "python3 -I conformance/tools/features_check.py --json", 30),
    ("build", "python3 -I conformance/tools/build_check.py --json", 90),
    ("matrix", "python3 -I conformance/tools/matrix_check.py --json", 30),
    ("acceptance-a", "python3 -I acceptance/run.py a", 60),
    ("acceptance-a2", "python3 -I acceptance/run.py a2", 60),
    ("plan", "python3 -I docs/delivery/tools/plan_check.py", 30),
]
TIERS = ("quick", "full")
# The quick tier runs every behavioural step: programs run on the interpreter, the VM and natively, and compared
# (execute, vm, native, parity, tests, packages), every documentation example on both checkers, the interpreter
# and the VM (docset), both acceptance applications on every backend, and the pilot's tests. It leaves out these
# compile-side and records-side steps, which the full tier runs at every release tag.
QUICK_SKIPS = {
    "cargo-build-debug": "the debug build (the full tier builds it, and compares it with the release build)",
    "genparser": "the generated parser against the recogniser, and its timing run (syntax and names still parse every fixture)",
    "codec": "the codec's golden files and round trips",
    "di": "the dependency injection kit",
    "frontend": "the production checker's records against the reference checker's on every fixture (checker, names and packages still judge them)",
    "batch": "fortisc check's batch mode and the query database's agreement with it",
    "querydb": "the query database (edits, corruption, cancellation, equality)",
    "view": "fortis view's queries and rendering",
    "lock": "the lockfile check",
    "features": "the feature sets, the sandboxed check and the debug-build comparison",
    "matrix": "the supported matrix (it reruns the features check)",
}
# The quick tier runs these in a lighter or split form: (command, what is left out or how it differs).
QUICK_FORMS = {
    "cargo-build-release": ("cd compiler && cargo build --release",
                            "the build with the query database's fault switches (qdb-faults)"),
    "docset": (f"python3 -I conformance/tools/docset_check.py --json --backends interpreter,vm -j {max(2, os.cpu_count() or 2)}",
               "the native runs of the documentation examples (both checkers, the interpreter and the VM still judge every example)"),
    "pilot": ("python3 -I conformance/tools/pilot_tests.py",
              "nothing: the same interpreter and VM tests, run module by module so that they can be split"),
    "build": ("python3 -I conformance/tools/build_check.py --json --no-clones",
              "the compiler and an application built from two clean clones (one package is still built from two directories)"),
}
# a quick form that builds nothing natively leaves the native lane
QUICK_NOT_NATIVE = {"docset", "pilot"}
# `--thorough` (the release workflow): larger budgets where a step takes one
THOROUGH = {
    "di": " --permutations 100 --seed 1009",
    "codec": " --runs 300",
}
# `--jobs`: the phases, in order; the steps of a phase run together, up to N at a time, except where a phase is
# marked alone. A step not named here is independent: it reads the syntax run's results at most.
PHASE_OF = {
    "unit": -1,                                    # fail before any build or corpus work
    "hygiene": 0, "cargo-build-release": 0, "cargo-build-debug": 0, "cargo-test": 0,  # alone: everything uses the build
    "syntax": 1,                                    # conformance/results.json, which most checks read
    "vm": 3, "native": 3, "shapes": 3,              # the interpreter's and the reference checker's results
    # alone: the steps that time themselves (genparser's parse timing, querydb's 50 ms cancellation budget), checks
    # that rerun other checks, a check that nothing else writes while it runs (view), and parity, which builds the
    # corpus natively as native does
    "genparser": 4, "querydb": 4, "features": 4, "matrix": 4, "view": 4, "parity": 4,
    "plan": 5,                                      # every result
}
ALONE = {-1, 0, 1, 4, 5}
INDEPENDENT = 2
# the steps that build programs natively (build/native-build, build/native-prod), run one after another
NATIVE_LANE = {"packages", "codec", "di", "adapters", "tests", "batch", "docset", "showcase", "pilot", "build", "limits", "fmt",
               "acceptance-a", "acceptance-a2"}


# `--shard`: the steps CI's build jobs run instead (compiler/target comes from the build job's artifact)
BUILD_STEPS = ("cargo-build-release", "cargo-build-debug", "cargo-test")
# what a step reads that another step writes, so the shard that runs it runs those first; every shard runs unit
# before any other work, then syntax, whose results (conformance/results.json) most checks read
PREREQS = {"vm": ["execute"], "native": ["execute"], "shapes": ["checker"], "parity": ["checker", "execute"],
           "matrix": ["execute"]}
# steps that can be split across shards: (the tool in the step's command that takes the option, the option)
SPLITS = {"docset": ("docset_check.py", "--shard {j}/{k}"), "pilot": ("pilot_tests.py", "--shard {j}/{k}"),
          "parity": ("parity_check.py", "--shard {j}/{k}")}
# at most this many parts: each part of parity brings the checker's and the interpreter's whole runs as prerequisites
SPLIT_MAX = {"parity": 2}


def splittable(name, cmd):
    return name in SPLITS and SPLITS[name][0] in cmd
TIMINGS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "gate-timings.json")
DEFAULT_SECONDS = 30


def load_timings(tier):
    try:
        return json.load(open(TIMINGS, encoding="utf-8")).get(tier, {})
    except (OSError, ValueError):
        return {}


THOROUGH_ON = False      # set by --thorough


def tier_steps(tier):
    """(name, command, minutes) of every step a tier runs, in order, with the quick tier's lighter forms and,
    under --thorough, the larger budgets."""
    out = []
    for name, cmd, minutes in STEPS:
        if tier == "quick" and name in QUICK_SKIPS:
            continue
        if tier == "quick" and name in QUICK_FORMS:
            cmd = QUICK_FORMS[name][0]
        if THOROUGH_ON and name in THOROUGH:
            cmd += THOROUGH[name]
        out.append((name, cmd, minutes))
    return out


# steps that need no compiled toolchain (the reference tools in Python, and the pilot's interpreter parts): with
# --py-shards P, they fill shards 1 to P, which CI starts at once, without waiting for its build job
PYTHON_ONLY = {"unit", "hygiene", "syntax", "names", "checker", "execute", "cfg", "aspects", "diagnostics", "library", "cite",
               "terms", "limitations", "plan"}


def python_only(part):
    base = part.split("[")[0]
    if base == "pilot" and "[" in part:
        return not part.startswith("pilot[1/")          # part 1 also runs the VM's tests
    return base in PYTHON_ONLY


def plan_shards(tier, n, py=0, build=0, selected=None):
    """The N shards of a tier: for each, its (name, command, minutes, role) in step order, role `own` or
    `prerequisite`. With py > 0, shards 1 to py take only steps that need no compiled toolchain. With build > 0,
    the toolchain arrives `build` seconds after a shard starts in a cold run (CI's build job; --toolchain), and
    WARM_BUILD seconds after in a warm one, and a shard's steps that need no toolchain run meanwhile: a shard ends
    at max(build, its python steps) plus its toolchain steps. The parts go to the shard whose cold and warm ends,
    added, grow least, so a shard with long prerequisites takes less of the rest. Deterministic for a given table
    of timings."""
    timings = load_timings(tier)
    steps = [st for st in tier_steps(tier) if st[0] not in BUILD_STEPS and (selected is None or st[0] in selected)]
    cost = {name: float(timings.get(name, DEFAULT_SECONDS)) for name, _, _ in steps}
    share = sum(cost.values()) / n
    order = {name: i for i, (name, _, _) in enumerate(steps)}
    parts = []          # (name, command, minutes, cost, order key)
    for name, cmd, minutes in steps:
        # a split step goes in parts of at most half a share, which pack well around the other steps
        k = min(n, SPLIT_MAX.get(name, n), max(1, -(-2 * int(cost[name]) // max(1, int(share))))) if splittable(name, cmd) else 1
        if k == 1:
            parts.append((name, cmd, minutes, cost[name], (order[name], 0)))
        else:
            for j in range(1, k + 1):
                parts.append((f"{name}[{j}/{k}]", cmd + " " + SPLITS[name][1].format(j=j, k=k), minutes, cost[name] / k, (order[name], j)))
    by_name = {name: (name, cmd, minutes) for name, cmd, minutes in steps}
    pyload, toolload = [0.0] * n, [0.0] * n     # each shard's seconds of steps that need no toolchain, and of the rest
    present = [set() for _ in range(n)]         # the steps a shard runs, its own and their prerequisites
    owned = [[] for _ in range(n)]

    def needs(name):
        base = name.split("[")[0]
        if base == "unit":
            return []
        return [p for p in ["unit"] + (["syntax"] + PREREQS.get(base, []) if base != "syntax" else []) if p in by_name]

    def adds(k, part):
        """(python seconds, toolchain seconds) that `part` adds to shard k, with the prerequisites it brings (all of
        them need no toolchain); a shard that already runs one saves it."""
        pre = sum(cost.get(p, DEFAULT_SECONDS) for p in needs(part[0]) if p not in present[k] and p in by_name)
        return (pre + part[3], 0.0) if python_only(part[0]) else (pre, part[3])

    def ends(k, add=(0.0, 0.0)):
        p, t = pyload[k] + add[0], toolload[k] + add[1]
        if not build:
            return p + t
        return (max(build, p) + t if t else p) + (max(min(build, WARM_BUILD), p) + t if t else p)

    # longest first, a step counted with the prerequisites it brings
    for part in sorted(parts, key=lambda x: (-(x[3] + sum(cost.get(p, DEFAULT_SECONDS) for p in needs(x[0]) if p != "syntax")), x[4])):
        pool = range(n) if not py else (range(py) if python_only(part[0]) else range(py, n))
        k = min(pool, key=lambda i: (ends(i, adds(i, part)), i))
        a = adds(k, part)
        pyload[k] += a[0]
        toolload[k] += a[1]
        present[k] |= {part[0]} | set(needs(part[0]))
        owned[k].append(part)
    shards = []
    for k in range(n):
        own = {x[0] for x in owned[k]}
        rows = [(x[0], x[1], x[2], "own", x[4]) for x in owned[k]]
        for p in sorted({q for x in owned[k] for q in needs(x[0]) if q in by_name and q not in own}):
            name, cmd, minutes = by_name[p]
            rows.append((name, cmd, minutes, "prerequisite", (order[name], 0)))
        shards.append([r[:4] for r in sorted(rows, key=lambda r: r[4])])
    return shards


def combine(paths, change_plan=None):
    """Check that the shards' reports cover their tier, each step once, and that every step passed."""
    reports = [json.load(open(x, encoding="utf-8")) for x in paths]
    tiers = {r.get("tier") for r in reports}
    shards = sorted(r.get("shard", "") for r in reports)
    problems = []
    if change_plan:
        expected = plan_shards("quick", change_plan["shards"], change_plan["py_shards"],
                               change_plan["build_seconds"], change_plan["steps"])
        by_shard = {f"{i}/{len(expected)}": {(nm, role) for nm, _, _, role in shard}
                    for i, shard in enumerate(expected, 1)}
        for report in reports:
            if report.get("plan") != change_plan:
                problems.append("a shard's plan differs from the requested plan")
            if report.get("tier") != change_plan["tier"]:
                problems.append("a shard's tier differs from the requested plan")
            if report.get("verdict") != "PASS":
                problems.append(f"shard {report.get('shard')} did not pass")
            for st in report.get("steps", []):
                need = NEEDS.get(st["step"])
                if st.get("status") == "skipped" and (not need or os.path.exists(os.path.join(os.path.dirname(__file__), "..", "..", need))):
                    problems.append(f"planned step {st['step']} was skipped")
            got = [(s["step"], s.get("role")) for s in report.get("steps", [])]
            if len(got) != len(set(got)) or set(got) != by_shard.get(report.get("shard")):
                problems.append(f"shard {report.get('shard')} did not run exactly its planned steps and prerequisites")
        if len(reports) != change_plan["shards"]:
            problems.append(f"expected {change_plan['shards']} shard reports, got {len(reports)}")
        if len({r.get("commit") for r in reports}) != 1:
            problems.append("the reports are from different commits")
    if len(tiers) != 1:
        problems.append(f"the reports are of different tiers: {sorted(map(str, tiers))}")
    tier = reports[0].get("tier") if reports else "quick"
    n = len(reports)
    if shards != sorted(f"{i}/{n}" for i in range(1, n + 1)):
        problems.append(f"the reports are shards {shards}, not 1/{n} to {n}/{n}")
    owners, rows = {}, []
    for r in reports:
        for st in r.get("steps", []):
            if st.get("role") == "prerequisite":
                if st["status"] not in ("pass", "skipped"):
                    problems.append(f"{st['step']} (a prerequisite in shard {r.get('shard')}) {st['status']}")
                continue
            base = st["step"].split("[")[0]
            owners.setdefault(base, []).append(st["step"])
            rows.append((st["step"], r.get("shard"), st["status"], st["seconds"]))
            if st["status"] not in ("pass", "skipped"):
                problems.append(f"{st['step']} (shard {r.get('shard')}) {st['status']}")
    want = [name for name, _, _ in tier_steps(tier) if name not in BUILD_STEPS
            and (change_plan is None or name in change_plan["steps"])]
    for name in want:
        if name not in owners:
            problems.append(f"{name} ran in no shard")
    for name in owners:
        if name not in want:
            problems.append(f"{name} ran but is not a step of the {tier} tier")
        parts = owners[name]
        if name in SPLITS and len(parts) > 1:
            k = len(parts)
            if sorted(parts) != sorted(f"{name}[{j}/{k}]" for j in range(1, k + 1)):
                problems.append(f"{name} ran as {sorted(parts)}, not as parts 1 to {k}")
        elif len(parts) != 1:
            problems.append(f"{name} ran {len(parts)} times")
    order = {nm: i for i, (nm, _, _) in enumerate(STEPS)}
    for step, shard, status, seconds in sorted(rows, key=lambda x: order.get(x[0].split("[")[0], len(order))):
        print(f"{status:<22} {seconds:>6} s  {step}  (shard {shard})")
    left = [f"{x}: quick tier: {QUICK_SKIPS[x]}" for x in QUICK_SKIPS] if tier == "quick" else []
    if change_plan:
        left += [f"{nm}: outside the change plan" for nm, _, _ in tier_steps(tier) if nm not in change_plan["steps"]]
    left += [f"{x}: run by CI's build and rust-tests jobs" for x in BUILD_STEPS
             if change_plan is None or x in change_plan["steps"]]
    print(f"not run in the shards ({tier} tier):")
    for x in left:
        print(f"    {x}")
    for x in problems:
        print(f"PROBLEM {x}")
    verdict = "PASS" if not problems else "FAIL"
    print(f"gate: {verdict} ({tier} tier, {n} shards combined; {len(want)} steps)" + ("; a quick pass is not a release pass" if tier == "quick" else ""))
    return verdict, tier, problems


def timings_from(paths):
    """Refresh gate-timings.json from gate reports (a shard's own steps; a split step's parts added up)."""
    table = json.load(open(TIMINGS, encoding="utf-8")) if os.path.exists(TIMINGS) else {}
    got = {}
    for x in paths:
        r = json.load(open(x, encoding="utf-8"))
        tier = r.get("tier", "full")
        for st in r.get("steps", []):
            if st.get("role") == "prerequisite" or st["status"] != "pass":
                continue
            base = st["step"].split("[")[0]
            got.setdefault(tier, {})[base] = got.get(tier, {}).get(base, 0) + st["seconds"]
    for tier, steps in got.items():
        table.setdefault(tier, {}).update(steps)
    with open(TIMINGS, "w", encoding="utf-8") as out:
        json.dump(table, out, indent=1, sort_keys=True)
        out.write("\n")
    print(f"gate: timings updated in {os.path.relpath(TIMINGS)}")


# steps that need a maintainers' planning tool: the step runs only when the file exists in the tree
NEEDS = {
    "limitations": "docs/delivery/tools/limitations.py",
    "plan": "docs/delivery/tools/plan_check.py",
}


RUN_TMP = None          # this run's own directory under build/tmp/ (main); each step gets a fresh one inside it
TOOLCHAIN_MINUTES = 30  # --toolchain's deadline
WARM_BUILD = 15         # with --build-seconds: when the toolchain arrives in a warm run, CI's build all cached


def run_step(tree, name, cmd, minutes):
    need = NEEDS.get(name)
    if need and not os.path.exists(os.path.join(tree, need)):
        why = f"{need} is a maintainers' planning tool and is not in this checkout"
        return {"step": name, "command": cmd, "status": "skipped", "seconds": 0, "tail": [why]}
    t = time.time()
    # the step's own temporary directory, removed when it ends
    tmp = tempfile.mkdtemp(prefix=re.sub(r"[^\w.-]", "_", name) + "-", dir=RUN_TMP)
    env = dict(os.environ, TMPDIR=tmp, TMP=tmp, TEMP=tmp)
    try:
        r = subprocess.run(["bash", "-c", cmd], cwd=tree, env=env, capture_output=True, text=True, timeout=minutes * 60)
        status = "pass" if r.returncode == 0 else f"fail (exit {r.returncode})"
        out = (r.stdout + r.stderr).strip().splitlines()
    except subprocess.TimeoutExpired as e:
        status = f"timeout after {minutes} min"
        out = ((e.stdout or b"").decode(errors="replace") + (e.stderr or b"").decode(errors="replace")).strip().splitlines()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    # what a step left out because a maintainers' planning file is not in this checkout (planning.py's lines)
    notes = [l for l in out if "maintainers' planning file" in l]
    return {"step": name, "command": cmd, "status": status, "seconds": round(time.time() - t), "tail": out[-15:], "notes": notes}


def run_phases(planned, one, jobs, lane):
    """Run the steps phase by phase (PHASE_OF); yield each result as it finishes. In a phase that runs steps
    together, the steps in `lane` run one after another, in order, as one task beside the others."""
    import concurrent.futures
    import queue
    phases = {}
    for st in planned:
        phases.setdefault(PHASE_OF.get(st[0], INDEPENDENT), []).append(st)
    for ph in sorted(phases):
        if ph in ALONE:
            for st in phases[ph]:
                yield one(st)
            continue
        finished = queue.Queue()
        in_lane = [st for st in phases[ph] if st[0] in lane]
        alone = [st for st in phases[ph] if st[0] not in lane]

        def guarded(st):
            # a step's own failure is its result; an error in the gate itself must not leave the wait below hanging
            try:
                return one(st)
            except Exception as e:  # noqa: BLE001
                return {"step": st[0], "command": st[1], "status": "fail (gate error)", "seconds": 0, "tail": [repr(e)]}

        def run_lane():
            for st in in_lane:
                finished.put(guarded(st))

        def run_one(st):
            finished.put(guarded(st))

        with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
            futures = ([pool.submit(run_lane)] if in_lane else []) + [pool.submit(run_one, st) for st in alone]
            for _ in range(len(in_lane) + len(alone)):
                yield finished.get()
            for fut in futures:
                fut.result()


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--tree", default=os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..")))
    ap.add_argument("--json")
    ap.add_argument("--from", dest="start")
    ap.add_argument("--only")
    ap.add_argument("--tier", choices=TIERS, default="full")
    ap.add_argument("--changed", metavar="BASE", help="quick tier: run what changes since BASE's merge base can affect")
    ap.add_argument("--ci-plan", metavar="JSON", help="run/combine the exact JSON plan supplied by CI's plan job")
    ap.add_argument("--jobs", default="auto", help="steps at once: a number, or auto (a quarter of the cores, at most 4; 1 in a shard)")
    ap.add_argument("--thorough", action="store_true", help="larger budgets where a step takes one (THOROUGH; the release workflow)")
    ap.add_argument("--shard", help="I/N: run the I-th of N shards (CI)")
    ap.add_argument("--py-shards", type=int, default=0, metavar="P", help="with --shard or --plan-shards: shards 1 to P take only the steps that need no compiled toolchain")
    ap.add_argument("--plan-shards", type=int, metavar="N", help="print how the tier's steps fall into N shards, and exit")
    ap.add_argument("--build-seconds", type=float, default=0, metavar="S", help="with --shard or --plan-shards: the toolchain shards' first S seconds go to "
                                                                              "CI's build, while their steps that need no toolchain run (--toolchain); every shard of a run must be given the same S")
    ap.add_argument("--combine", nargs="+", metavar="REPORT", help="check that shard reports cover the tier and passed")
    ap.add_argument("--toolchain", metavar="CMD", help="with --shard: a command that makes the compiled toolchain available (CI's fetches the build "
                                                       "job's artifact); the steps that need no toolchain run first, and CMD runs, under its own "
                                                       f"{TOOLCHAIN_MINUTES}-minute deadline, before the first step that needs it")
    ap.add_argument("--timings-from", nargs="+", metavar="REPORT", help="refresh gate-timings.json from reports")
    a = ap.parse_args(argv)
    change_plan = None
    if a.changed or a.ci_plan:
        if a.changed and a.ci_plan or a.start or a.only or a.thorough or a.tier != "quick":
            ap.error("--changed/--ci-plan requires --tier quick, without --from, --only or --thorough")
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import ci_plan
        change_plan = ci_plan.from_diff(a.tree, a.changed) if a.changed else json.loads(a.ci_plan)
        ci_plan.describe(change_plan)
        a.py_shards, a.build_seconds = change_plan["py_shards"], change_plan["build_seconds"]
    selected = change_plan["steps"] if change_plan else None
    global THOROUGH_ON
    THOROUGH_ON = a.thorough
    if a.jobs == "auto":
        a.jobs = 1 if a.shard else max(1, min(4, (os.cpu_count() or 1) // 4))
    else:
        a.jobs = int(a.jobs)
    if a.timings_from:
        timings_from(a.timings_from)
        return 0
    if a.combine:
        verdict, tier, problems = combine(a.combine, change_plan)
        if a.json:
            os.makedirs(os.path.dirname(os.path.abspath(a.json)), exist_ok=True)
            report = {"tier": tier, "verdict": verdict, "problems": problems, "shards": a.combine}
            if change_plan:
                report["plan"] = change_plan
            json.dump(report, open(a.json, "w"), indent=1)
        return 0 if verdict == "PASS" else 1
    if a.plan_shards:
        timings = load_timings(a.tier)
        for i, shard in enumerate(plan_shards(a.tier, a.plan_shards, a.py_shards, a.build_seconds, selected), 1):
            secs = {True: 0.0, False: 0.0}
            for nm, _, _, _ in shard:
                secs[python_only(nm)] += float(timings.get(nm.split("[")[0], DEFAULT_SECONDS)) / (int(nm.split("/")[1][:-1]) if "[" in nm else 1)
            end = max(a.build_seconds, secs[True]) + secs[False] if secs[False] else secs[True]
            warm = max(min(a.build_seconds, WARM_BUILD), secs[True]) + secs[False] if secs[False] else secs[True]
            print(f"shard {i}/{a.plan_shards} (about {end:.0f} s" + (f", {warm:.0f} s warm" if a.build_seconds else "") + "): "
                  + ", ".join(nm + (" (prerequisite)" if role != "own" else "") for nm, _, _, role in shard))
        return 0
    steps = [st for st in STEPS if selected is None or st[0] in selected]
    roles = {}
    if a.shard:
        if a.start or a.only or not re.fullmatch(r"[1-9]\d*/[1-9]\d*", a.shard) or int(a.shard.split("/")[0]) > int(a.shard.split("/")[1]):
            ap.error("--shard takes I/N with 1 <= I <= N, and no --from or --only")
        i, n = (int(x) for x in a.shard.split("/"))
        if change_plan and n != change_plan["shards"]:
            ap.error("--shard denominator differs from the plan")
        mine = plan_shards(a.tier, n, a.py_shards, a.build_seconds, selected)[i - 1]
        steps = [(nm, cmd, minutes) for nm, cmd, minutes, _ in mine]
        roles = {nm: role for nm, _, _, role in mine}
    if a.start:
        steps = steps[[s[0] for s in steps].index(a.start):]
    if a.only:
        keep = set(a.only.split(","))
        steps = [s for s in steps if s[0] in keep]
    try:
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=a.tree, capture_output=True, text=True, timeout=60).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        head = ""
    head = head or "(not a git checkout)"
    if a.shard:
        print(f"gate: tier {a.tier}, shard {a.shard}: " + ", ".join(nm + (" (prerequisite)" if roles[nm] != "own" else "") for nm, _, _ in steps)
              + "; `--combine` over every shard's report gives the tier's verdict", flush=True)
    elif change_plan:
        print(f"gate: tier quick: {len(steps)} steps selected by the change plan; a release needs the full tier", flush=True)
    elif a.tier == "quick":
        print(f"gate: tier quick: {len(QUICK_SKIPS)} steps left out and {len(QUICK_FORMS)} reduced (listed at the end); "
              f"a release needs the full tier", flush=True)
    else:
        print("gate: tier full: every step", flush=True)
    reduced, planned = [], []
    for name, cmd, minutes in steps:
        if a.shard:
            if a.tier == "quick" and name.split("[")[0] in QUICK_FORMS and roles[name] == "own":
                reduced.append({"step": name, "why": f"quick tier leaves out {QUICK_FORMS[name.split('[')[0]][1]}"})
        elif a.tier == "quick" and name in QUICK_FORMS:
            cmd, left_out = QUICK_FORMS[name]
            reduced.append({"step": name, "why": f"quick tier leaves out {left_out}"})
        if not a.shard and THOROUGH_ON and name in THOROUGH:
            cmd += THOROUGH[name]
        planned.append((name, cmd, minutes))

    # --toolchain: the steps that need no compiled toolchain first, while CI's build job is still running
    toolchain = {}
    if a.toolchain:
        if not a.shard:
            ap.error("--toolchain goes with --shard")
        planned = [st for st in planned if python_only(st[0])] + [st for st in planned if not python_only(st[0])]
    toolchain_lock = threading.Lock()

    def fetch_toolchain():
        """Run --toolchain once, under its deadline: True when the toolchain is there, else why not."""
        with toolchain_lock:
            if "ready" not in toolchain:
                t = time.time()
                try:
                    r = subprocess.run(["bash", "-c", a.toolchain], cwd=a.tree, capture_output=True, text=True, timeout=TOOLCHAIN_MINUTES * 60)
                    tail = (r.stdout + r.stderr).strip().splitlines()[-5:]
                    toolchain["ready"] = True if r.returncode == 0 else f"`{a.toolchain}` exited {r.returncode}: " + " / ".join(tail)
                except subprocess.TimeoutExpired:
                    tail = []
                    toolchain["ready"] = f"`{a.toolchain}` passed its {TOOLCHAIN_MINUTES}-minute deadline"
                toolchain["seconds"] = round(time.time() - t)
                print(f"gate: toolchain {'ready' if toolchain['ready'] is True else 'missing'} after {toolchain['seconds']} s"
                      + "".join(f"\n    {line}" for line in tail), flush=True)
            return toolchain["ready"]

    def one(step):
        name, cmd, minutes = step
        if a.tier == "quick" and name in QUICK_SKIPS:
            return {"step": name, "command": cmd, "status": "skipped", "seconds": 0, "tail": [f"quick tier: {QUICK_SKIPS[name]}"]}
        if a.toolchain and not python_only(name):
            ready = fetch_toolchain()
            if ready is not True:
                return {"step": name, "command": cmd, "status": "fail (no toolchain)", "seconds": 0, "tail": [ready]}
        return run_step(a.tree, name, cmd, minutes)

    started = time.time()
    global RUN_TMP
    os.makedirs(os.path.join(a.tree, "build", "tmp"), exist_ok=True)
    RUN_TMP = tempfile.mkdtemp(prefix="gate-", dir=os.path.join(a.tree, "build", "tmp"))
    if a.jobs <= 1:
        done = (one(st) for st in planned)
    else:
        lane = NATIVE_LANE - (QUICK_NOT_NATIVE if a.tier == "quick" else set())
        done = run_phases(planned, one, a.jobs, lane)
    by_name = {}
    try:
        for res in done:
            name = res["step"]
            by_name[name] = res
            print(f"{res['status']:<22} {res['seconds']:>6} s  {name}", flush=True)
            if res["status"] == "skipped":
                print(f"    skipped: {res['tail'][0]}", flush=True)
            elif res["status"] == "pass":
                for line in res.get("notes", []):
                    print(f"    {line}", flush=True)
            if res["status"] not in ("pass", "skipped"):
                for line in res["tail"]:
                    print(f"    {line}", flush=True)
                if name == "unit":
                    for pending, cmd, _ in planned:
                        if pending not in by_name:
                            by_name[pending] = {"step": pending, "command": cmd, "status": "not run (unit failed)",
                                                "seconds": 0, "tail": ["unit tests failed; stopped before further work"]}
                    break
    finally:
        shutil.rmtree(RUN_TMP, ignore_errors=True)
    results = [by_name[name] for name, _, _ in planned]
    for r in results:
        if roles:
            r["role"] = roles[r["step"]]
    ok = all(r["status"] in ("pass", "skipped") for r in results)
    skipped = [{"step": r["step"], "why": r["tail"][0]} for r in results if r["status"] == "skipped"]
    report = {"tree": a.tree, "commit": head, "tier": a.tier, "verdict": "PASS" if ok else "FAIL", "skipped": skipped,
              "reduced": reduced, "jobs": a.jobs, "thorough": THOROUGH_ON, "seconds": round(time.time() - started), "steps": results}
    if a.shard:
        report["shard"] = a.shard
    if change_plan:
        report["plan"] = change_plan
    if "seconds" in toolchain:
        report["toolchain_seconds"] = toolchain["seconds"]
    if a.json:
        os.makedirs(os.path.dirname(os.path.abspath(a.json)), exist_ok=True)
        json.dump(report, open(a.json, "w"), indent=1)
    if skipped or reduced:
        print(f"not run in this gate ({a.tier} tier):")
        for x in skipped:
            print(f"    skipped {x['step']}: {x['why']}")
        for x in reduced:
            print(f"    reduced {x['step']}: {x['why']}")
    ran = len(results) - len(skipped)
    print(f"gate: {report['verdict']} ({a.tier} tier) at {head if head.startswith('(') else head[:12]} "
          f"({sum(r['status'] == 'pass' for r in results)}/{ran} steps passed"
          + (f"; {len(skipped)} skipped" if skipped else "") + (f"; {len(reduced)} reduced" if reduced else "")
          + f"; {report['seconds']} s" + (f" with up to {a.jobs} at once" if a.jobs > 1 else "") + ")"
          + ("; a quick pass is not a release pass" if a.tier == "quick" else ""))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
