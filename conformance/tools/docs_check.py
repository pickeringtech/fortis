"""DEL-43 criterion 2 (documentation part): the documentation examples of the G1 subset run as fixtures through the
production toolchain (PROG-TST-08, Programme 8.1).

`python3 -I conformance/tools/docs_check.py [--json]`

The examples are the `fortis` blocks of the three documents (Language Reference, Platform Specification, Overview),
extracted with their markers into `conformance/corpus` (conformance/tools/extract.py; `extract.py --check` keeps the
corpus equal to the documents). A block is in the G1 subset when its stage is `core` (it imports no kit) and none
of its features is excluded by `docs/delivery/subsets.json`. A block importing an example package (`acme.…`) is the
root module of a package depending on the documentation packages (`conformance/doc-packages`), and is checked and
run as that package. For each G1 block:

- `// accept`: `fortisc check` reports no error; a module with `main` runs on the VM to a normal completion (no
  dynamic error, defect or refusal); a module with tests passes `fortisc test`; a module without either builds to
  validated typed IR (`fortisc mir`). A statement fragment is pasted into its context
  (`conformance/doc-contexts/<ID>.ft`, which declares only what the example names) and checked there.
- `// execute: R`: as accept, and the VM's observed result is `R`.
- `// reject: CODE`: `fortisc check` reports `CODE` at every marked line (anywhere when no line is marked), and no
  other error; a fragment's lines are mapped back from its context.

The remaining core-stage blocks are run by DEL-40's parity run and the kit-stage blocks by their groups.
"""
import json
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
sys.path.insert(0, HERE)
from vm_check import features, g1_subset  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")


CONTEXTS = os.path.join(ROOT, "conformance", "doc-contexts")


def in_context(fid, src):
    """(program text, line offset) for a fragment pasted into its context, or None without one."""
    path = os.path.join(CONTEXTS, fid + ".ft")
    if not os.path.exists(path):
        return None
    ctx = open(path, encoding="utf-8").read().split("\n")
    out = []
    for l in ctx:
        m = re.match(r"^//\s*base:\s*(\S+)", l)
        if m:
            base = open(os.path.join(ROOT, "conformance", "corpus", m.group(1) + ".ft"), encoding="utf-8").read().split("\n")
            out.extend(base[1:])           # the base's declarations, without its marker
        elif l.strip() == "// {{example}}":
            offset = len(out)
            out.extend("  " + x if x else x for x in src.rstrip("\n").split("\n"))
        else:
            out.append(l)
    return "\n".join(out) + "\n", offset


PACKAGES = os.path.join(ROOT, "conformance", "doc-packages")


def as_package(fid, src):
    """The block as the root module of a package beside the documentation packages; returns its directory."""
    search = os.path.join(ROOT, "build", "doc-packages", fid)
    shutil.rmtree(search, ignore_errors=True)
    for pkg in sorted(os.listdir(PACKAGES)):
        if not os.path.isdir(os.path.join(PACKAGES, pkg)):
            continue
        for root, _, files in os.walk(os.path.join(PACKAGES, pkg)):
            for f in files:
                text = open(os.path.join(root, f), encoding="utf-8").read()
                m = re.match(r"^//\s*base:\s*(\S+)", text)
                if m:
                    text = "\n".join(open(os.path.join(ROOT, "conformance", "corpus", m.group(1) + ".ft"), encoding="utf-8").read().split("\n")[1:])
                dest = os.path.join(search, os.path.relpath(os.path.join(root, f), PACKAGES))
                os.makedirs(os.path.dirname(dest), exist_ok=True)
                open(dest, "w", encoding="utf-8").write(text)
    uses = sorted({m.group(1) for m in re.finditer(r"^import\s+(acme\.[a-z]+)", src, re.M)})
    module = "app" if "src/app.ft" in src.split("\n")[0] else "example"
    root = os.path.join(search, "doc.example")
    os.makedirs(os.path.join(root, "src"))
    entry = f"entry: {module}\n" if re.search(r"func\s+main\s*\(", src) else ""
    deps = "".join(f'  {u}: "1.0.0"\n' for u in uses)
    open(os.path.join(root, "project.yaml"), "w", encoding="utf-8").write(f'schema: fortis.project/v1\nname: doc.example\nversion: 1.0.0\n{entry}exports: [{module}]\ndependencies:\n  fortis: "1.0"\n{deps}')
    open(os.path.join(root, "src", module + ".ft"), "w", encoding="utf-8").write(src)
    return root


def fortisc(*args):
    r = subprocess.run([FORTISC] + list(args), capture_output=True, text=True, cwd=ROOT, timeout=1800)
    return r.returncode, r.stdout, r.stderr


def main(argv):
    manifest = json.load(open(os.path.join(ROOT, "conformance", "corpus", "manifest.json"), encoding="utf-8"))["fixtures"]
    excluded = g1_subset()
    rows, problems = [], 0
    for fx in manifest:
        path = fx["path"]
        src = open(os.path.join(ROOT, path), encoding="utf-8").read()
        feats = features(src)
        row = {"id": fx["id"], "doc": fx["doc"], "line": fx["doc_line"], "verdict": fx["verdict"], "stage": fx["stage"], "features": feats}
        if fx["verdict"] == "illustrative":
            row["status"] = "illustrative"
        elif fx["stage"] != "core":
            row["status"] = "kit stage (its group)"
        elif set(feats) & excluded:
            row["status"] = f"core, outside G1 ({', '.join(sorted(set(feats) & excluded))}): DEL-40's parity run"
        else:
            head = subprocess.run([FORTISC, "syntax", path], capture_output=True, text=True, cwd=ROOT).stdout.split("\t")
            form = head[2].strip() if len(head) > 2 else ""
            target, offset, where = path, 0, ""
            if "package" in feats:
                target, where = as_package(fx["id"], src), " (as a package)"
            elif form != "module":
                placed = in_context(fx["id"], src)
                if placed is None:
                    rows.append({**row, "status": "run", "ok": False, "detail": f"a fragment without a context in conformance/doc-contexts/{fx['id']}.ft"})
                    problems += 1
                    continue
                text, offset = placed
                target = os.path.join(ROOT, "build", "doc-contexts", fx["id"] + ".ft")
                os.makedirs(os.path.dirname(target), exist_ok=True)
                open(target, "w", encoding="utf-8").write(text)
                where = " (in its context)"
            code, out, _ = fortisc("check", "--format", "json", target)
            recs = [json.loads(l) for l in out.splitlines() if l.startswith("{")]
            errors = [dict(r, line=r["line"] - offset) for r in recs if r["severity"] == "error"]
            ok, detail = False, ""
            if fx["verdict"] == "reject":
                marked = [(m["line"], m["code"]) for m in fx["marked_lines"]]
                found = {(r["line"], r["code"]) for r in errors}
                ok = (all(m in found for m in marked) and found <= set(marked)) if marked else (found and {c for _, c in found} <= set(fx["codes"]))
                detail = f"expected {marked or fx['codes']}, reported {sorted(found)}{where}"
            elif errors:
                detail = "; ".join(f"{r['code']}@{r['line']}: {r['message']}" for r in errors[:3]) + where
            else:
                has_main = re.search(r"^\s*(export\s+)?(async\s+)?(try\s+)?func\s+main\s*\(", src, re.M) is not None
                has_tests = re.search(r"^\s*(test|property)\s+\"", src, re.M) is not None
                if form != "module":
                    ok, detail = True, "checked in its context, no errors"
                elif has_main:
                    _, vout, _ = fortisc("vm", target)
                    observed = next((l[len("observed: "):] for l in vout.splitlines() if l.startswith("observed: ")), "")
                    bad = observed.startswith(("dynamic:", "refused", "unbuilt", "refused-ir")) or "defect:" in observed
                    want = re.search(r"^//\s*execute:\s*(.*)$", src, re.M)
                    ok = (observed == want.group(1).strip()) if (fx["verdict"] == "execute" and want) else not bad
                    detail = f"main ran: {observed[:120]!r}"
                elif has_tests:
                    tcode, tout, _ = fortisc("test", "--format", "json", target)
                    summary = [json.loads(l) for l in tout.splitlines() if '"test-summary"' in l]
                    ok = tcode == 0 and summary and summary[-1]["failed"] == 0
                    detail = f"tests: {summary[-1] if summary else tout[:200]}"
                else:
                    mcode, _, merr = fortisc("mir", target)
                    ok, detail = mcode == 0, "built to validated typed IR" if mcode == 0 else merr[:200]
            row.update(status="run", ok=bool(ok), detail=detail)
            problems += not ok
        rows.append(row)
    run_rows = [r for r in rows if r["status"] == "run"]
    for r in run_rows:
        print(f"{'PASS' if r['ok'] else 'FAIL'} {r['id']:<12} {r['verdict']:<8} {r['detail'][:150]}")
    by = {}
    for r in rows:
        key = r["status"].split(" (")[0] if r["status"] != "run" else "run"
        by[key] = by.get(key, 0) + 1
    print(f"\n{len(rows)} documentation blocks: {len(run_rows)} in the G1 subset run through the production toolchain, {sum(r['ok'] for r in run_rows)} pass "
          f"({sum(r['verdict'] == 'accept' for r in run_rows)} accept, {sum(r['verdict'] == 'execute' for r in run_rows)} execute, {sum(r['verdict'] == 'reject' for r in run_rows)} reject); "
          + "; ".join(f"{k}: {v}" for k, v in sorted(by.items()) if k != "run"))
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rule": "PROG-TST-08", "blocks": rows}, open(os.path.join(ROOT, "build", "docs.json"), "w"), indent=1)
    print("docs check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
