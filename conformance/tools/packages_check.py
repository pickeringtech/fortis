"""Runs the resolver over every package fixture in conformance/packages that has an expect.json and
writes conformance/packages/results.json (results keyed by fixture id and backend `names`). A reject fixture's
optional `at` list positions its records: `package/src/module.ft:LINE`, relative to conformance/packages."""
import hashlib
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import diagnostics  # noqa: E402
import parser   # noqa: E402
import project    # noqa: E402
import resolve  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, ROOT)
from checker import check  # noqa: E402
PK = os.path.join(ROOT, "conformance", "packages")


def pinned(records):
    """Records as a fixture's expect.json `records` pins them, in a canonical order: file name, line, column, code,
    message and candidates. A fixture that pins its records must produce exactly those (frontend_check.py holds the
    production frontend to the same list)."""
    out = [{"file": os.path.basename(r.get("file", "")), "line": r.get("line"), "column": r.get("column"), "code": r.get("code"),
            "message": r.get("message"), "candidates": r["candidates"] if "candidates" in r else (r.get("fields") or {}).get("candidates")}
           for r in records]
    return sorted(out, key=lambda x: json.dumps(x, sort_keys=True))


def _observed(out):
    return next((l[len("observed: "):] for l in out.splitlines() if l.startswith("observed: ")), "")


def judge_runfile(name, d, expect):
    """A module file under `src/` is the package (D-327): a package error refuses and writes no binary, and a valid
    module runs the package program. Returns a list of problems."""
    files = expect["runfile"]
    if isinstance(files, str):
        files = [files]
    fortis = [sys.executable, "-I", os.path.join(ROOT, "bin", "fortis")]
    fortisc = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
    problems = []
    out_root = os.path.join(ROOT, "build", "runfile")
    os.makedirs(out_root, exist_ok=True)

    def run(args, timeout):
        return subprocess.run(args, capture_output=True, text=True, cwd=ROOT, timeout=timeout)

    if expect["verdict"] == "reject":
        code = expect.get("code") or (expect.get("codes") or [""])[0]
        denied = expect.get("must_not_print")
        for rel in files:
            target = os.path.join(d, rel)
            got = run(fortis + ["run", target], 120)
            blob = got.stdout + got.stderr
            if got.returncode == 0 or code not in blob or (denied and denied in got.stdout.splitlines()):
                problems.append(f"fortis run {rel}: exit {got.returncode} out {got.stdout!r} err {got.stderr[-200]!r}")
            vm = run([fortisc, "vm", target], 180)
            obs = _observed(vm.stdout)
            if vm.returncode == 0 or not obs.startswith("refused:" + code) or (denied and denied in obs.split("|")):
                problems.append(f"fortisc vm {rel}: exit {vm.returncode} observed {obs!r}")
            outbin = os.path.join(out_root, name + "-" + rel.replace("/", "_"))
            if os.path.lexists(outbin):
                os.remove(outbin)
            bld = run([fortisc, "build", "--out", outbin, target], 600)
            if bld.returncode == 0 or os.path.exists(outbin):
                problems.append(f"fortisc build {rel}: exit {bld.returncode} binary {os.path.exists(outbin)}")
            nat = run([fortisc, "native", target], 600)
            nobs = _observed(nat.stdout)
            if not nobs.startswith("refused:" + code) or (denied and denied in nobs.split("|")):
                problems.append(f"fortisc native {rel}: exit {nat.returncode} observed {nobs!r}")
        return problems

    def interp(path):
        return run(fortis + ["run", path], 120)

    def vm(path):
        return run([fortisc, "vm", path], 180)

    def native(path):
        return run([fortisc, "native", path], 600)

    def built(path, tag):
        outbin = os.path.join(out_root, tag)
        if os.path.lexists(outbin):
            os.remove(outbin)
        bld = run([fortisc, "build", "--out", outbin, path], 600)
        if bld.returncode != 0 or not os.path.exists(outbin):
            return bld.returncode, None, (bld.stdout + bld.stderr)[-400:]
        ran = run([outbin], 60)
        return ran.returncode, ran.stdout, ran.stderr

    def origins(text):
        found_rows = []
        for line in text.splitlines():
            line = line.strip()
            if line.startswith("record: "):
                line = line[len("record: "):].strip()
            if not line.startswith("{"):
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("kind") == "runtime-failure":
                found_rows.append(rec.get("origin"))
        return found_rows

    suffix = expect.get("origin_suffix")
    found = {}

    def note(label, text):
        if suffix:
            found[label] = origins(text)

    base_i, base_v, base_n = interp(d), vm(d), native(d)
    base_b = built(d, name + "-dir")
    note("interpreter dir", base_i.stderr)
    note("vm dir", base_v.stderr)
    note("native dir", base_n.stdout)
    note("build dir", base_b[2] or "")
    want = expect.get("expected")
    got_lines = "|".join(base_i.stdout.splitlines())
    if base_i.returncode != 0 or (want is not None and got_lines != want):
        problems.append(f"package interpreter: exit {base_i.returncode} {got_lines!r} want {want!r} {base_i.stderr[-200]!r}")
    if base_v.returncode != 0 or (want is not None and _observed(base_v.stdout) != want):
        problems.append(f"package vm: exit {base_v.returncode} {_observed(base_v.stdout)!r}")
    if base_n.returncode != 0 or (want is not None and _observed(base_n.stdout) != want):
        problems.append(f"package native: exit {base_n.returncode} {_observed(base_n.stdout)!r}")
    if base_b[1] is None or base_b[0] != 0 or (want is not None and "|".join(base_b[1].splitlines()) != want):
        problems.append(f"package build: exit {base_b[0]} out {base_b[1]!r} {base_b[2]!r}")
    for rel in files:
        target = os.path.join(d, rel)
        one_i = interp(target)
        if (one_i.returncode, one_i.stdout) != (base_i.returncode, base_i.stdout):
            problems.append(f"fortis run {rel}: exit {one_i.returncode} {one_i.stdout!r} {one_i.stderr[-200]!r}")
        one_v = vm(target)
        if one_v.returncode != base_v.returncode or _observed(one_v.stdout) != _observed(base_v.stdout):
            problems.append(f"fortisc vm {rel}: exit {one_v.returncode} {_observed(one_v.stdout)!r}")
        one_n = native(target)
        if one_n.returncode != base_n.returncode or _observed(one_n.stdout) != _observed(base_n.stdout):
            problems.append(f"fortisc native {rel}: exit {one_n.returncode} {_observed(one_n.stdout)!r}")
        one_b = built(target, name + "-" + rel.replace("/", "_"))
        if one_b[0] != base_b[0] or one_b[1] != base_b[1]:
            problems.append(f"fortisc build {rel}: exit {one_b[0]} out {one_b[1]!r} {one_b[2]!r}")
        if suffix:
            note(f"interpreter {rel}", one_i.stderr)
            note(f"vm {rel}", one_v.stderr)
            note(f"native {rel}", one_n.stdout)
            note(f"build {rel}", one_b[2] or "")
    if suffix:
        groups = list(found.values())
        flat = [o for group in groups for o in group]
        same = bool(groups) and all(group == groups[0] and group for group in groups)
        files_ok = all(isinstance(o, dict) and str(o.get("file", "")).endswith(suffix) for o in flat)
        if not (same and files_ok and flat):
            shown = {k: v[:2] for k, v in found.items()}
            problems.append(f"origin file: suffix {suffix!r} {shown}")
    return problems


def dir_hash(d):
    h = hashlib.sha256()
    for base, _, files in sorted(os.walk(d)):
        for f in sorted(files):
            p = os.path.join(base, f)
            h.update(os.path.relpath(p, d).encode()); h.update(open(p, "rb").read())
    return h.hexdigest()


def main(argv):
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    results, failures = [], 0
    for name in sorted(os.listdir(PK)):
        d = os.path.join(PK, name)
        ex = os.path.join(d, "expect.json")
        if not os.path.exists(ex):
            continue
        expect = json.load(open(ex, encoding="utf-8"))
        ps = None
        name_only = False
        try:
            ps = resolve.PackageSet(d, PK, rec)
            records = ps.run()
            name_only = bool(records)
        except project.ManifestError as e:
            records = [{"code": "E-NAM-001", "message": str(e), "line": 1}]
        if ps is not None and not records:
            try:
                records = check.check_package_set(ps)     # names, declarations and visibility per module (DEL-07, DEL-10)
            except Exception as e:  # noqa: BLE001
                records = [{"code": "E-INT-000", "message": f"checker failure: {e!r}", "line": 1}]
            name_only = False
        codes = sorted({r["code"] for r in records})
        # an execute fixture is statically an accept here; its run is judged by the backends (release_results.py); a
        # reject fixture names its code, or `codes` when it expects several
        want = sorted(expect.get("codes") or [expect.get("code")]) if expect["verdict"] == "reject" else []
        expected = "accept" if expect["verdict"] in ("accept", "execute") else "reject:" + ",".join(want)
        observed = "accept" if not records else "reject:" + ",".join(codes)
        ok = (expected == observed) if expect["verdict"] in ("accept", "execute") else all(c in codes for c in want)
        if ok and "records" in expect and pinned(records) != pinned(expect["records"]):
            ok = False
            print(f"     PKG-{name}: records differ from the pinned ones: {pinned(records)}")
        # `at` positions a reject: each `package/path.ft:LINE` (relative to conformance/packages) must carry a record of
        # an expected code, and every record of those codes must be at one of them
        at = expect.get("at", [])
        if at and expect["verdict"] == "reject":
            places = {(os.path.join("conformance", "packages", a.rsplit(":", 1)[0]), int(a.rsplit(":", 1)[1])) for a in at}
            have = {(r.get("file"), r.get("line")) for r in records if r["code"] in want}
            ok = ok and places == have
            expected += " at " + ",".join(at)
            observed = expected if ok else observed + " at " + ",".join(f"{f}:{ln}" for f, ln in sorted(have, key=str))
        if ok and "onefile" in expect:
            # a module of this package checked on its own (D-51.8): both production CLIs must give, for the file,
            # the package check's findings for it plus the manifest-level records, and nothing else. Checked
            # alone, the file's imports resolve against nothing and the missing member is accepted silently.
            target = os.path.join(d, expect["onefile"])
            keep = (os.path.basename(expect["onefile"]), "project.yaml")
            kept = [r for r in records if os.path.basename(r.get("file", "")) in keep]
            # the package check stopped on another module's syntax error, so this file has no record of its own.
            # Both CLIs then report that the module was not name-checked (bin/fortis `package_file`).
            if not kept and any(str(r.get("code", "")).startswith("E-SYN") for r in records):
                kept = [diagnostics.make("E-NAM-001", resolve.NOT_NAME_CHECKED, "LANG-04-03", "project.yaml", 1, 1, {"name": "", "candidates": []})]
            elif not kept and name_only and ps is not None:
                # a sibling's name error skipped the type pass. The one-file check still type-checks this file
                # (bin/fortis `package_file`). A clean file stays an empty filter.
                try:
                    typed = check.check_package_set(ps)
                except Exception as e:  # noqa: BLE001
                    typed = [{"code": "E-INT-000", "message": f"checker failure: {e!r}", "line": 1, "file": "project.yaml"}]
                kept = [r for r in typed if os.path.basename(r.get("file", "")) in keep]
            onefile = pinned(kept)
            for who, cmd in (("fortis", [sys.executable, os.path.join(ROOT, "bin", "fortis")]),
                             ("fortisc", [os.path.join(ROOT, "compiler", "target", "release", "fortisc")])):
                got = subprocess.run(cmd + ["check", target, "--format", "json"], capture_output=True, text=True, cwd=ROOT, timeout=600)
                one = [json.loads(line) for line in got.stdout.splitlines() if line.strip()]
                if pinned(one) != onefile:
                    ok = False
                    print(f"     PKG-{name}: {who} check {expect['onefile']} gives {pinned(one)}, the package check gives {onefile}")
        if ok and "runfile" in expect:
            # a file under src/ uses the package check and the package program (D-327)
            run_problems = judge_runfile(name, d, expect)
            if run_problems:
                ok = False
                print(f"     PKG-{name}: runfile: {run_problems[0]}")
        status = "PASS" if ok else "FAIL"
        failures += not ok
        results.append({"id": f"PKG-{name}", "path": os.path.relpath(d, ROOT), "verdict": expect["verdict"], "codes": want or ([expect.get("code")] if expect.get("code") else []),
                        "covers": expect.get("covers", []), "stage": "core", "packages": [], "status": status, "backend": "names", "commit": commit,
                        "sha256": dir_hash(d), "command": "python3 -I conformance/tools/packages_check.py", "expected": expected, "observed": observed,
                        # the results are committed: a message's paths are written relative to the repository root
                        "diagnostics": [dict(r, message=r["message"].replace(ROOT + os.sep, "")) if isinstance(r.get("message"), str) else r
                                        for r in records]})
        print(f"{status:4} PKG-{name:<20} expected {expected:<18} observed {observed}")
    json.dump({"results": results}, open(os.path.join(PK, "results.json"), "w", encoding="utf-8"), indent=2)
    print(f"\n{len(results)} package fixtures, {failures} FAIL")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
