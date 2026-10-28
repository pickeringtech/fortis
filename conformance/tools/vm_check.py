"""The development VM against the definitional interpreter (DEL-35).

Runs every `// execute:` fixture on the VM (`fortisc vm`: the typed IR of fortis-mir executed by fortis-vm) and
compares, fixture by fixture, what the interpreter observed (conformance/execution/results.json, written by
execute_check.py): the console lines and final `fail:`/`defect:` marker, the `execute-trace` marker when the
fixture has one, and the kinds and codes of the records the run emitted. Each fixture is classified by the
features it uses; the G1 subset (docs/delivery/subsets.json) excludes defer, async, scope, pools, host
capabilities other than the console, codecs, defects, kit modules and other packages.

  python3 -I conformance/tools/vm_check.py [--json] [-v]"""
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
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")

FEATURES = [
    ("defer", re.compile(r"^\s*defer\b", re.M)),
    ("async", re.compile(r"\basync\b|\bawait\b|\bTask\[")),
    ("scope", re.compile(r"^\s*scope\s+\w+", re.M)),
    ("pools", re.compile(r"\blaunch(Parallel|Blocking)?\(")),
    ("host", re.compile(r"\bcaps\.(fileSystem|network|clock|random|environment|process|supervisor|secretAccess|rawSql)\b|\b(FileSystem|Network|Clock|Random|Environment|Process|RawSql|Path)\b")),
    ("codec", re.compile(r"\.(toText|toBytes|fromText|fromBytes)\(")),
    ("defects", re.compile(r"\binjectDefect\b|defect:")),
    ("kit", re.compile(r"^import\s+fortis\.(?!(prelude|text|collections|numeric|time|money|host)\b)", re.M)),
    ("package", re.compile(r"^import\s+(?!fortis\.)\w", re.M)),
]


def code_only(src: str) -> str:
    return "\n".join(l.split("//")[0] for l in src.split("\n"))


def features(src: str) -> list:
    body = code_only(src)
    out = [n for n, rx in FEATURES if rx.search(body if n != "defects" else src)]
    # the kernel's defined behaviour for an unavailable kit (a `dynamic:kit not available` failure) is
    # itself G1 behaviour: such a fixture does not use the kit
    if "kit" in out and re.search(r"^// execute: dynamic:kit not available", src, re.M):
        out.remove("kit")
    return out


def g1_subset():
    sub = json.load(open(os.path.join(ROOT, "docs", "delivery", "subsets.json"), encoding="utf-8"))
    return set(sub["G1"]["excluded features"])


def marked_tests(path):
    """An execute fixture asks the harness to run its tests (`// tests: run`). The user-facing `vm` and
    `native` commands do not read that comment (D-326): a comment must not stop `main`. The harness passes
    `--tests` only for an execute fixture. An accept program that mentions the marker still runs `main`,
    which is what parity and the native accept batch compare."""
    try:
        text = open(path if os.path.isabs(path) else os.path.join(ROOT, path), encoding="utf-8").read()
    except OSError:
        return False
    lines = [line.lstrip() for line in text.splitlines()]
    return any(line.startswith("// tests: run") for line in lines) and any(line.startswith("// execute:") for line in lines)


def run_split(cmd, paths, env=None, timeout=7200):
    """`fortisc cmd` on `paths`. Fixtures carrying the harness marker are a second invocation with `--tests`."""
    if not paths:
        return ""
    tests = [p for p in paths if marked_tests(p)]
    rest = [p for p in paths if p not in tests]
    kw = dict(capture_output=True, text=True, cwd=ROOT, timeout=timeout)
    if env is not None:
        kw["env"] = env
    out = []
    if rest:
        out.append(subprocess.run([FORTISC, cmd] + rest, **kw).stdout)
    if tests:
        out.append(subprocess.run([FORTISC, cmd, "--tests"] + tests, **kw).stdout)
    return "".join(out)


def main(argv):
    verbose = "-v" in argv
    interp = {r["path"]: r for r in json.load(open(os.path.join(ROOT, "conformance", "execution", "results.json"), encoding="utf-8"))["results"]}
    paths = sorted(interp)
    out = run_split("vm", paths, timeout=3600)
    vm, cur = {}, None
    for line in out.splitlines():
        if line.startswith("=== "):
            cur = line[4:]
            vm[cur] = {"observed": None, "trace": "", "records": []}
        elif line.startswith("observed: "):
            vm[cur]["observed"] = line[len("observed: "):]
        elif line.startswith("trace: "):
            vm[cur]["trace"] = line[len("trace: "):]
        elif line.startswith("record: "):
            rec = dict(kv.split("=", 1) for kv in line[len("record: "):].split("; ") if "=" in kv)
            vm[cur]["records"].append(rec)
    excluded = g1_subset()
    rows = []
    for p in paths:
        src = open(os.path.join(ROOT, p), encoding="utf-8").read()
        feats = features(src)
        tests = re.search(r"^//\s*tests:\s*run", src, re.M) is not None
        r = interp[p]
        v = vm.get(p, {"observed": "missing", "trace": "", "records": []})
        problems = []
        want = r["observed"]
        if v["observed"] != want:
            problems.append(f"output: interpreter {want!r}, VM {v['observed']!r}")
        tm = re.search(r"^//\s*execute-trace:\s*(.*)$", src, re.M)
        if tm and v["trace"] != tm.group(1).strip():
            problems.append(f"trace: expected {tm.group(1).strip()!r}, VM {v['trace']!r}")
        kinds = ("runtime-failure", "defect", "observation-frame", "observation-event", "test-result", "buffer-overflow")
        key = lambda x: (x.get("kind"), x.get("code", x.get("class", x.get("status", x.get("dropped", "")))))
        ir = [key(x) for x in r.get("reports", []) if x.get("kind") in kinds]
        vr = [key(x) for x in v["records"] if x.get("kind") in kinds]
        ir = [(k, str(c)) for k, c in ir]
        vr = [(k, str(c)) for k, c in vr]
        if ir != vr:
            problems.append(f"records: interpreter {ir}, VM {vr}")
        g1 = not (set(feats) & excluded)
        rows.append({"path": p, "features": feats, "g1": g1, "tests": tests, "agree": not problems, "problems": problems})
    g1rows = [x for x in rows if x["g1"]]
    for x in rows:
        if not x["agree"] and (verbose or x["g1"]):
            print(f"{'G1 ' if x['g1'] else '   '}DISAGREE {x['path']} [{', '.join(x['features'])}]")
            for pr in x["problems"]:
                print(f"      {pr[:300]}")
    print(f"\nexecute fixtures: {len(rows)}, {sum(x['agree'] for x in rows)} agree")
    print(f"G1 subset: {len(g1rows)} fixtures, {sum(x['agree'] for x in g1rows)} agree, {sum(not x['agree'] for x in g1rows)} disagree")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build", "vm"), exist_ok=True)
        json.dump({"rows": rows}, open(os.path.join(ROOT, "build", "vm", "results.json"), "w"), indent=1)
    return 0 if all(x["agree"] for x in g1rows) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
