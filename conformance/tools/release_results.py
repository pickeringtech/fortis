"""DEL-44 (DW-02): every fixture linked to a `LANG-` rule, judged on each release backend at this commit.

`python3 -I conformance/tools/release_results.py [-j N]` writes conformance/release/results.json, which
`plan_check.py --release` reads with the other result files (results keyed by fixture and backend).

The release backends are the three implementations (PROG-TST-06/07):

- **interpreter**: the reference toolchain (`bin/fortis`): the reference checker judges `accept` and `reject`; the
  definitional interpreter runs `execute` fixtures (a program's result comes from execute_check.py; an `execute`
  package, which execute_check.py does not run, is run here with `bin/fortis run`).
- **vm**: the production frontend and the development VM (`fortisc`): `fortisc check` judges, an accepted module
  must build to validated typed IR (`fortisc mir`), and `fortisc vm` runs `execute` fixtures.
- **native**: the production frontend and the native backend: an accepted module must build to an executable
  (`fortisc build`), a rejected one must be refused before lowering, and `fortisc native` runs `execute` fixtures.

`expected` is `accept`, `reject:` and the declared codes, or the declared output; `observed` is what the backend did
(for `reject`, the declared codes it reported at their marked lines). A statement fragment of the documents is judged
in its context (conformance/doc-contexts) and a block importing an example package as a package
(conformance/doc-packages), as docs_check.py does.
"""
import concurrent.futures
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
import docs_check  # noqa: E402
import native_check  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
REFERENCE = [sys.executable, "-I", os.path.join(ROOT, "bin", "fortis")]
OUT = os.path.join(ROOT, "conformance", "release", "results.json")
WORK = os.path.join(ROOT, "build", "release")
# the interpreter's execute results (execute_check.py), for the native defect rule
INTERP = {r["path"]: r for r in json.load(open(os.path.join(ROOT, "conformance", "execution", "results.json"), encoding="utf-8"))["results"]} \
    if os.path.exists(os.path.join(ROOT, "conformance", "execution", "results.json")) else {}


def sha(path):
    full = os.path.join(ROOT, path)
    if os.path.isdir(full):
        h = hashlib.sha256()
        for base, _, files in sorted(os.walk(full)):
            for f in sorted(files):
                p = os.path.join(base, f)
                h.update(os.path.relpath(p, full).encode())
                h.update(open(p, "rb").read())
        return h.hexdigest()
    return hashlib.sha256(open(full, encoding="utf-8").read().encode("utf-8")).hexdigest()


def linked():
    """Every fixture linked to a LANG rule: (id, path, verdict, declared codes, marked (line, code) pairs, expected output)."""
    out = {}
    for fx in json.load(open(os.path.join(ROOT, "conformance", "corpus", "manifest.json"), encoding="utf-8"))["fixtures"]:
        if fx["stage"] == "core" and re.search(r"LANG-\d\d-\d\d", fx.get("note", "")) and fx["verdict"] != "illustrative":
            out[fx["id"]] = (fx["path"], fx["verdict"], fx["codes"], [(m["line"], m["code"]) for m in fx["marked_lines"]], None)
    for name in sorted(os.listdir(os.path.join(ROOT, "conformance", "programs"))):
        if not name.endswith(".ft"):
            continue
        path = os.path.join("conformance", "programs", name)
        src = open(os.path.join(ROOT, path), encoding="utf-8").read()
        if not re.search(r"//\s*covers:.*LANG-\d\d-\d\d", src):
            continue
        if re.search(r"^import\s+fortis\.(?!(prelude|text|collections|numeric|time|money|host)\b)", src, re.M):
            continue                                   # a kit-stage fixture: its group's release
        m = re.match(r"//\s*(accept|reject|execute|illustrative)\b\s*:?\s*(.*)", src)
        verdict = m.group(1) if m else "accept"
        codes = sorted(set(re.findall(r"E-[A-Z]+-\d+", m.group(2)))) if m and verdict == "reject" else []
        # a marked line may name several codes: those between `reject:` and the dash that starts the explanation
        marked = [(i, c) for i, l in enumerate(src.split("\n"), start=1) if not l.lstrip().startswith("//") and re.search(r"//\s*reject:", l)
                  for c in re.findall(r"E-[A-Z]+-\d+", re.split(r"\s—\s|\s-\s", l.split("reject:", 1)[1], maxsplit=1)[0])]
        expected = m.group(2).strip() if verdict == "execute" else None
        out["PRG-" + name[:-3]] = (path, verdict, codes, marked, expected)
    pk = os.path.join(ROOT, "conformance", "packages")
    for name in sorted(os.listdir(pk)):
        ex = os.path.join(pk, name, "expect.json")
        if os.path.exists(ex):
            e = json.load(open(ex, encoding="utf-8"))
            if any(r.startswith("LANG-") for r in e.get("covers", [])):
                out["PKG-" + name] = (os.path.join("conformance", "packages", name), e["verdict"], [e["code"]] if e.get("code") else [], [], e.get("expected"))
    return out


def run(args, cwd=ROOT, timeout=900):
    r = subprocess.run(args, capture_output=True, text=True, cwd=cwd, timeout=timeout)
    return r.returncode, r.stdout, r.stderr


def target_of(fid, path, verdict):
    """The file or package a backend judges: the fixture itself, its fragment in context, or its package."""
    src = open(os.path.join(ROOT, path), encoding="utf-8").read() if os.path.isfile(os.path.join(ROOT, path)) else ""
    if not path.startswith("conformance/corpus/"):
        return os.path.join(ROOT, path), 0
    feats = docs_check.features(src)
    if "package" in feats:
        return docs_check.as_package(fid, src), 0
    _, so, _ = run([FORTISC, "syntax", path])
    form = so.split("\t")[2].strip() if len(so.split("\t")) > 2 else ""
    if form != "module":
        placed = docs_check.in_context(fid, src)
        if placed is None:
            return None, 0
        text, offset = placed
        target = os.path.join(WORK, "contexts", fid + ".ft")
        os.makedirs(os.path.dirname(target), exist_ok=True)
        open(target, "w", encoding="utf-8").write(text)
        return target, offset
    return os.path.join(ROOT, path), 0


def codes_of(stdout, offset):
    found = set()
    for l in stdout.splitlines():
        if l.startswith("{"):
            try:
                r = json.loads(l)
            except json.JSONDecodeError:
                continue
            if r.get("severity", "error") == "error" and "code" in r:
                found.add((r["line"] - offset, r["code"]))
    return found


# codes of the build-time plan step (PROG-VM-04), which `check` never reports: judged where a program is run or built
BUILD_TIME = {"E-EFF-030"}


def judge_static(found, verdict, codes, marked):
    if verdict in ("accept", "execute"):
        return "accept" if not found else "reject:" + ",".join(sorted({c for _, c in found}))
    # each marked code at its line; a declared code no line marks may be reported anywhere
    marked_codes = {c for _, c in marked}
    hit = sorted({c for (ln, c) in marked if (ln, c) in found} | {c for c in codes if c not in marked_codes and any(c == fc for _, fc in found)})
    return "reject:" + ",".join(hit) if hit else ("accept" if not found else "reject-other:" + ",".join(sorted({c for _, c in found})))


def one(fid, entry, commit):
    path, verdict, codes, marked, expected_out = entry
    target, offset = target_of(fid, path, verdict)
    rows = []
    expected = "accept" if verdict == "accept" else ("reject:" + ",".join(sorted(codes)) if verdict == "reject" else (expected_out or ""))
    digest = sha(path)

    def row(backend, observed, command):
        rows.append({"id": fid, "path": path, "verdict": verdict, "backend": backend, "status": "PASS" if observed == expected else "FAIL", "commit": commit,
                     "sha256": digest, "command": command, "expected": expected, "observed": observed, "stage": "core", "packages": []})

    if target is None:
        for be in ("interpreter", "vm", "native"):
            row(be, "no context for the fragment", "")
        return rows
    rel = os.path.relpath(target, ROOT)
    build_time = verdict == "reject" and codes and set(codes) <= BUILD_TIME
    if build_time:
        # the plan step: refused by `run` on the interpreter, `vm` on the VM and `build` natively
        _, so, _ = run(REFERENCE + ["run", "--format", "json", rel])
        row("interpreter", judge_static(codes_of(so, offset), verdict, codes, marked), f"bin/fortis run --format json {rel}")
        _, so, _ = run([FORTISC, "vm", rel])
        refused = {(ln, c) for (ln, c) in marked} if any(l.startswith("observed: refused:") and l.split("refused:")[1] in codes for l in so.splitlines()) else set()
        row("vm", judge_static(refused, verdict, codes, marked), f"fortisc vm {rel}")
        _, so, _ = run([FORTISC, "build", "--format", "json", "--out", os.path.join(WORK, "bin", fid), rel], timeout=1800)
        row("native", judge_static(codes_of(so, offset), verdict, codes, marked), f"fortisc build --format json {rel}")
        return rows
    # interpreter: the reference checker; an execute program's run is execute_check.py's result, and an execute
    # package (which execute_check.py does not run) is run here with `bin/fortis run`
    if verdict != "execute":
        _, so, _ = run(REFERENCE + ["check", "--format", "json", rel])
        row("interpreter", judge_static(codes_of(so, offset), verdict, codes, marked), f"bin/fortis check --format json {rel}")
    elif os.path.isdir(target):
        code, so, _ = run(REFERENCE + ["run", rel])
        row("interpreter", "|".join(so.splitlines()) if code == 0 else f"exit {code}: " + "|".join(so.splitlines()), f"bin/fortis run {rel}")
    # vm: the production check, validated IR, and the VM for an execute fixture
    _, so, _ = run([FORTISC, "check", "--format", "json", rel])
    static = judge_static(codes_of(so, offset), verdict, codes, marked)
    if verdict == "reject" or static != "accept":
        row("vm", static, f"fortisc check --format json {rel}")
    elif verdict == "execute":
        _, so, _ = run([FORTISC, "vm", rel])
        obs = next((l[len("observed: "):] for l in so.splitlines() if l.startswith("observed: ")), "no run")
        row("vm", obs, f"fortisc vm {rel}")
    else:
        form_module = rel.endswith(".ft") and run([FORTISC, "syntax", rel])[1].split("\t")[2:3] == ["module"]
        if os.path.isdir(target) or form_module:
            code, _, se = run([FORTISC, "mir", rel])
            row("vm", "accept" if code == 0 else "unbuilt: " + se.strip()[:120], f"fortisc mir {rel}")
        else:
            row("vm", "accept", f"fortisc check --format json {rel}")
    # native: refused before lowering, built to an executable, or run
    if verdict == "execute":
        _, so, _ = run([FORTISC, "native", rel], timeout=1800)
        obs = next((l[len("observed: "):] for l in so.splitlines() if l.startswith("observed: ")), "no run")
        # a defect aborts the process natively and ends only the task on the other backends (10.8): the native
        # expectation is the interpreter's lines up to the defect, then the defect (native_check.py's rule)
        interp = INTERP.get(path)
        if obs != expected and interp is not None and native_check.native_defect_consequence(interp, {"observed": obs}, os.path.join(ROOT, path)):
            rows.append({"id": fid, "path": path, "verdict": verdict, "backend": "native", "status": "PASS", "commit": commit, "sha256": digest,
                         "command": f"fortisc native {rel}", "expected": obs, "observed": obs, "stage": "core", "packages": [],
                         "note": f"the interpreter's {expected!r} up to the defect, then the process aborts (Language Reference 10.8)"})
        else:
            row("native", obs, f"fortisc native {rel}")
    else:
        exe = os.path.join(WORK, "bin", fid)
        os.makedirs(os.path.dirname(exe), exist_ok=True)
        code, so, se = run([FORTISC, "build", "--format", "json", "--out", exe, rel], timeout=1800)
        found = codes_of(so, offset)
        if verdict == "reject":
            row("native", judge_static(found, verdict, codes, marked), f"fortisc build --format json {rel}")
        else:
            row("native", "accept" if code == 0 else (judge_static(found, verdict, codes, marked) if found else "unbuilt: " + (se or so).strip()[-120:]), f"fortisc build {rel}")
    return rows


def main(argv):
    jobs = int(argv[argv.index("-j") + 1]) if "-j" in argv else max(2, (os.cpu_count() or 4) // 2)
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    fixtures = linked()
    # the runtime crates are built once before the parallel builds share them
    run([FORTISC, "build", "--format", "json", "--out", os.path.join(WORK, "bin", "warm"), "conformance/programs/eff_noalloc_ok.ft"], timeout=1800)
    rows = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        for r in pool.map(lambda kv: one(kv[0], kv[1], commit), sorted(fixtures.items())):
            rows.extend(r)
    rows.sort(key=lambda r: (r["id"], r["backend"]))
    # a failure a release-blocking defect of the defect register reproduces is listed against it (the release
    # stays blocked: such a result is not PASS); any other failure fails this run. The register is a maintainers'
    # planning file: without it no failure is KNOWN, so every failure fails the run.
    import planning
    defects_path = planning.optional(planning.DEFECTS, "release-results", "listing failures against the open defects (every failure fails)")
    defects = json.load(open(defects_path, encoding="utf-8"))["defects"] if defects_path else []
    known = {d["id"]: d for d in defects if d.get("release_blocking") and d["status"] != "fixed"}
    # a defect names the results it reproduces: `"results": {"fixtures": [ids], "backends": [names]}`
    for r in rows:
        for d in known.values():
            rs = d.get("results", {})
            if r["status"] != "PASS" and r["id"] in rs.get("fixtures", []) and r["backend"] in rs.get("backends", []):
                r["status"], r["defect"] = "KNOWN", d["id"]
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    json.dump({"commit": commit, "results": rows}, open(OUT, "w", encoding="utf-8"), indent=1)
    fails = [r for r in rows if r["status"] not in ("PASS", "KNOWN")]
    for r in rows:
        if r["status"] == "KNOWN":
            print(f"KNOWN {r['id']} on {r['backend']}: {r['observed'][:100]!r} (release-blocking defect {r['defect']}: {known[r['defect']]['title']})")
    for r in fails:
        print(f"FAIL {r['id']} on {r['backend']}: expected {r['expected']!r}, observed {r['observed'][:160]!r}")
    by = {}
    for r in rows:
        by.setdefault(r["backend"], [0, 0])[r["status"] == "PASS"] += 1
    print(f"\n{len(fixtures)} linked fixtures; " + "; ".join(f"{b}: {v[1]} pass, {v[0]} fail" for b, v in sorted(by.items())))
    print("release results: " + ("OK" if not fails else "FAILED"))
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
