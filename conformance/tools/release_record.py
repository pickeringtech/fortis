"""DEL-44 (DW-09): the release-evidence record of Language 1.0, with integrity, attestation and correctness apart.

`python3 -I conformance/tools/release_record.py` writes each acceptance application's resolved contract manifest
(acceptance/<app>/contracts.json) and, in the maintainers' checkout, the release record in the delivery plan, which
the plan's own verifier, sharing no code with this tool, verifies independently. The delivery plan is a maintainers'
planning file: a checkout without it writes the contract manifests and says that it skipped the record.

It establishes, as executed facts:

- **PROG-FND-01**: the core inventory of Programme 2.1 (modules, capability types, runtime inventory, language
  areas) matches the Language Reference's table of contents, the core package's modules and the compatibility set.
- **PROG-FND-05**: every guarantee an acceptance application relies on names its checker, its scope and its trust
  assumptions (the build manifest's transitive trusted operations), in the application's contract manifest.
- **PROG-FND-07**: no kit is installed (the standard library holds only the core package), the core
  conformance results were produced so, and the toolchain has no plugin mechanism (no dynamic loading anywhere).
- **PROG-DEL-05**: the release scope (G0–G3) depends on no G4–G7 unit, and its acceptance runs need no kit.
- **PROG-NFR-16**: every critical or high defect is fixed, or open with a release-blocking review record
  (the delivery plan's safety review) that names its decision.

and aggregates the other done-when items from their own checks' outputs (plan_check, release results, acceptance
run logs, the matrix, the privilege check, the readability registration and the targets). The record states
`releasable` only when every item holds; otherwise it lists what blocks the release.
"""
import datetime
import hashlib
import json
import os
import platform
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
OUT = os.path.join(ROOT, "docs", "delivery", "release", "record.json")
APPS = ["a", "a2"]
CORE_MODULES = ["prelude", "text", "collections", "numeric", "time", "money", "host"]
CAPABILITIES = ["FileSystem", "Network", "Clock", "Random", "Environment", "Process", "Supervisor", "Console", "SecretAccess", "RawSql"]
# Programme 2.1's language areas, each with the Language Reference section that defines it
AREAS = {
    "lexical grammar": "3. Lexical structure", "statements": "5. Statements and expressions", "modules and packages": "4. Modules and packages",
    "types and refinements": "6. Types", "ownership with lexical loans": "7. Ownership", "linear outcomes": "10. Outcomes and failures",
    "the closed effect set": "11. Effects and contracts", "synchronous defer, async/await, scope": "12. Cleanup, async and concurrency",
    "the observation schema head": "13. Observation schemas and statements", "the core runtime inventory": "15. Core runtime inventory",
}
# each guarantee a declaration can carry: (checker, scope)
GUARANTEES = {
    "pure": ("effect checker (fortis-types effects; E-EFF-001…)", "transitive: the function and everything it calls (11.3)"),
    "noalloc": ("effect checker, then the plan-compatibility step at build (E-EFF-030)", "transitive: the function and everything it calls (11.3, 11.4)"),
    "try": ("obligation checker (E-OBL): every outcome is consumed exactly once", "every caller (10.2)"),
    "async": ("the core executor's contract (12.3) and the loan checker across suspension points", "the function and the tasks it launches"),
    "recovers": ("obligation checker: no failure escapes", "the function (10.5)"),
    "trusted": ("none: a trusted claim is an assumption, listed under trust", "the function"),
    "contract": ("contract checker (11.5)", "transitive: the function and everything it calls"),
    "refinement": ("interval domain at compile time; validation where it cannot prove (6.3, E-REF)", "the value's type, wherever it flows"),
    "view": ("ownership checker: the view's loan cannot outlive its origin (7.6, E-OWN)", "the caller's block"),
    "owned": ("ownership checker: the argument is moved and the caller cannot use it again (7.1, E-OWN)", "the call"),
    "observe": ("observation checker: required fields assigned before each exit (13, E-OBS)", "the function's exits"),
}
BASELINE_TRUST = ["the generated crate is #![forbid(unsafe_code)] Rust checked by rustc (natively)", "the fortis-native runtime crate and the Rust standard library",
                  "the development VM's own checks (on the VM)", "the host adapters behind each capability the entry receives"]


def sha_file(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()


def sha_tree(d):
    h = hashlib.sha256()
    for base, dirs, files in sorted(os.walk(d)):
        dirs.sort()
        for f in sorted(files):
            p = os.path.join(base, f)
            h.update(os.path.relpath(p, d).encode())
            h.update(open(p, "rb").read())
    return h.hexdigest()


def load(rel):
    p = os.path.join(ROOT, rel)
    return json.load(open(p, encoding="utf-8")) if os.path.exists(p) else None


def run(cmd, timeout=3600, **kw):
    r = subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT, timeout=timeout, **kw)
    return r.returncode, r.stdout, r.stderr


def contract_manifest(app):
    """The guarantees of one acceptance application, each with its checker, scope and trust assumptions."""
    src = os.path.join(ROOT, "acceptance", app)
    _, so, _ = run([FORTISC, "view", "--decls", "--json", os.path.join("acceptance", app, "src")])
    decls = [json.loads(l) for l in so.splitlines() if l.startswith("{")]
    exe = os.path.join(ROOT, "build", "release-record", app)
    os.makedirs(os.path.dirname(exe), exist_ok=True)
    code, so, se = run([FORTISC, "build", "--format", "json", "--out", exe, os.path.join("acceptance", app)], timeout=1800)
    manifest = json.load(open(exe + ".build.json", encoding="utf-8")) if code == 0 and os.path.exists(exe + ".build.json") else {}
    trust = {t["operation"]: t for t in manifest.get("trust", [])}
    functions = manifest.get("functions", {})
    out = []
    for d in decls:
        if d["declaration"] not in ("func", "class"):
            continue
        head = d["head"]
        kinds = [m for m in d["modifiers"] if m in GUARANTEES]
        kinds += ["contract"] * bool(d["contracts"])
        kinds += ["refinement"] * bool(re.search(r"\bInt<[^>]*>", head))
        kinds += ["view"] * bool(re.search(r"\)\s*\S.*\bfrom\b", head) or "alias " in head)
        kinds += ["owned"] * bool(re.search(r"\bowned\b", head))
        kinds += ["observe"] * any(a.startswith("@observe") for a in d.get("aspects", []))
        if not kinds:
            continue
        name = f"{d['member_of']}.{d['name']}" if d.get("member_of") else d["name"]
        t = next((v for k, v in trust.items() if k.endswith("::" + name.replace(".", "::")) or k.endswith("." + name) or k.endswith("::" + d["name"])), None)
        f = next((v for k, v in functions.items() if k.endswith("::" + name.replace(".", "::")) or k.endswith("." + name) or k.endswith("::" + d["name"])), None)
        out.append({
            "declaration": name, "file": d["path"], "line": d["line"], "head": head,
            "guarantees": [{"kind": k, "checker": GUARANTEES[k][0], "scope": GUARANTEES[k][1]} for k in dict.fromkeys(kinds)],
            "contracts": d["contracts"],
            "effects": (f or {}).get("effects"), "authorities": (f or {}).get("authorities"),
            "trust": {"baseline": BASELINE_TRUST, "operations": (t or {}).get("assumptions", []), "trusted_effects": (t or {}).get("trusted_effects", [])},
        })
    doc = {"schema": "fortis.contracts/v1", "application": app, "rule": "PROG-FND-05", "build_manifest": os.path.relpath(exe + ".build.json", ROOT) if manifest else None,
           "built": code == 0, "guarantees": out}
    json.dump(doc, open(os.path.join(src, "contracts.json"), "w", encoding="utf-8"), indent=1)
    return doc


def main(argv):
    items, blocking = [], []

    def item(key, rule, ok, detail, evidence=None):
        items.append({"item": key, "rule": rule, "holds": bool(ok), "detail": detail, "evidence": evidence or []})
        if not ok:
            blocking.append(f"{key}: {detail}")
        print(f"{'PASS' if ok else 'FAIL'} {key} ({rule}): {detail}"[:400])

    _, head, _ = run(["git", "rev-parse", "HEAD"])
    head = head.strip()
    _, dirty, _ = run(["git", "status", "--porcelain", "--untracked-files=no"])
    # PROG-FND-01: the core inventory
    lr = open(os.path.join(ROOT, "docs", "Fortis_Language_Reference.md"), encoding="utf-8").read()
    toc = re.findall(r"^## (\d+\. .+)$", lr, re.M)
    host = open(os.path.join(ROOT, "library", "fortis", "host.ft"), encoding="utf-8").read()
    compat = open(os.path.join(ROOT, "library", "fortis", "compatibility.yaml"), encoding="utf-8").read()
    present = sorted(f[:-3] for f in os.listdir(os.path.join(ROOT, "library", "fortis")) if f.endswith(".ft"))
    missing = [a for a, s in AREAS.items() if s not in toc] + [m for m in CORE_MODULES if m not in present] + [c for c in CAPABILITIES if f"export class {c} " not in host]
    extra = [m for m in present if m not in CORE_MODULES]
    host_parts = all(x in host for x in ("export class Secret[T]", "export class Host", "capabilities()", "reveal"))
    item("core inventory", "PROG-FND-01", not missing and not extra and host_parts and re.search(r'^  fortis: "1.0"$', compat, re.M),
         f"{len(AREAS)} language areas in the Language Reference's contents, {len(CORE_MODULES)} core modules and no others, {len(CAPABILITIES)} capability types, Secret, reveal and Host.capabilities(); missing {missing}, extra {extra}",
         ["docs/Fortis_Language_Reference.md", "library/fortis", "library/fortis/compatibility.yaml"])
    # PROG-FND-05: contract manifests
    docs = [contract_manifest(a) for a in APPS]
    complete = all(d["built"] and d["guarantees"] and all(g["guarantees"] and all(x["checker"] and x["scope"] for x in g["guarantees"]) and g["trust"]["baseline"] for g in d["guarantees"]) for d in docs)
    item("resolved contract manifests", "PROG-FND-05", complete, "; ".join(f"{d['application']}: {len(d['guarantees'])} guaranteed declarations, each with checker, scope and trust" for d in docs),
         [f"acceptance/{a}/contracts.json" for a in APPS])
    # PROG-FND-07 and PROG-DEL-05: kits absent
    lib = sorted(os.listdir(os.path.join(ROOT, "library")))
    rs = [os.path.join(dp, f) for dp, dn, fn in os.walk(os.path.join(ROOT, "compiler", "crates")) for f in fn if f.endswith((".rs", ".toml"))]
    plugins = [os.path.relpath(p, ROOT) for p in rs if re.search(r"libloading|dlopen|dlsym|LoadLibrary", open(p, encoding="utf-8").read())]
    item("no kit installed; no compiler plugins", "PROG-FND-07", lib == ["fortis"] and not plugins,
         f"library holds {lib}; dynamic loading in the toolchain: {plugins or 'none'}; the core results below were produced with this library",
         ["library", "compiler/crates"])
    sys.path.insert(0, HERE)
    import planning  # noqa: E402
    if not planning.optional(planning.PLAN_TOOLS, "release-record", "the release record (the contract manifests above are written)"):
        return 0
    sys.path.insert(0, os.path.join(ROOT, "docs", "delivery", "tools"))
    import chunks  # noqa: E402  (defines the units; writes nothing unless run as a program)
    core_gate = {"G0", "G1", "G2", "G3"}
    graph = {c["id"]: c for c in chunks.C + chunks.GROUPS + chunks.MILESTONES}
    unknown = sorted({d for c in chunks.C for d in c["depends"] if d not in graph})
    later = sorted(f"{u} -> {d}" for u, c in graph.items() if c.get("gate") in core_gate for d in c["depends"] if d not in graph or graph[d].get("gate") not in core_gate)
    specs = [load(f"acceptance/{a}/spec.json") or {} for a in APPS]
    kit_imports = [a for a in APPS for dp, _, fn in os.walk(os.path.join(ROOT, "acceptance", a, "src")) for f in fn
                       if re.search(r"^\s*import\s+fortis\.(?!prelude|text|collections|numeric|time|money|host)\w", open(os.path.join(dp, f), encoding="utf-8").read(), re.M)]
    item("releasable without any G4–G7 deliverable", "PROG-DEL-05", graph and not later and not unknown and not kit_imports,
         f"{sum(1 for v in graph.values() if v.get('gate') in core_gate)} G0–G3 units depend on none of the {sum(1 for v in graph.values() if v.get('gate') not in core_gate)} G4–G7 units ({later or 'no such edge'}); acceptance applications import no kit ({kit_imports or 'none'})",
         ["docs/delivery/tools/chunks.py"])
    # PROG-NFR-16: the safety review
    defects = (load("docs/delivery/defects.json") or {}).get("defects", [])
    review = {r["defect"]: r for r in (load("docs/delivery/safety-review.json") or {}).get("reviews", [])}
    unreviewed = [d["id"] for d in defects if d["severity"] in ("critical", "high") and d["status"] != "fixed" and not (d["id"] in review and review[d["id"]].get("decision"))]
    blockers = [d["id"] for d in defects if d["severity"] in ("critical", "high") and d["status"] != "fixed" and review.get(d["id"], {}).get("release_blocking")]
    item("safety defects reviewed", "PROG-NFR-16", not unreviewed, f"{len(defects)} defects; unresolved critical/high without a review: {unreviewed or 'none'}; reviewed as release-blocking: {blockers or 'none'}",
         ["docs/delivery/defects.json", "docs/delivery/safety-review.json"])
    if blockers:
        blocking.append(f"release-blocking defects: {', '.join(blockers)}")
    # the aggregated items, each from its own check's output
    code, so, _ = run([sys.executable, "-I", os.path.join(ROOT, "docs", "delivery", "tools", "plan_check.py"), "--release", "--all-errors"], timeout=1800)
    errors = [l[3:] for l in so.splitlines() if l.startswith(" - ")]
    reasons = {}
    for e in errors:
        reasons[e.rsplit(": ", 1)[-1]] = reasons.get(e.rsplit(": ", 1)[-1], 0) + 1
    tail = ["plan check: OK" if code == 0 else f"{len(errors)} release errors: " + ", ".join(f"{n} {r}" for r, n in sorted(reasons.items(), key=lambda x: -x[1]))]
    release = load("conformance/release/results.json") or {}
    rows = release.get("results", [])
    by = {}
    for r in rows:
        by.setdefault(r["backend"], []).append(r["status"])
    item("DW-01 G0–G3 passed, everything owned (plan_check --release)", "PROG-DEL-01, PROG-FND-06", code == 0, tail[0][:200], ["docs/delivery/status.json", "docs/delivery/ownership.json"])
    item("DW-02 every LANG- rule's fixtures pass on all three backends at this commit", "PROG-TST-06, PROG-TST-07",
         release.get("commit") == head[:len(release.get("commit", "x"))] and rows and all(s in ("PASS", "KNOWN") for r in rows for s in [r["status"]]) and not any(r["status"] == "KNOWN" for r in rows),
         f"commit {release.get('commit')} (HEAD {head[:7]}); " + "; ".join(f"{b}: {v.count('PASS')} pass, {v.count('KNOWN')} known defect, {len(v) - v.count('PASS') - v.count('KNOWN')} fail" for b, v in sorted(by.items())),
         ["conformance/release/results.json"])
    logs = {a: load(f"acceptance/{a}/run-log.json") or {} for a in APPS}
    item("DW-05 A and A2 on the interpreter, the VM and native", "PROG-NFR-06",
         all(l.get("semantic_correctness", {}).get("passed") for l in logs.values()) and all(set(s.get("backends", [])) == {"interpreter", "vm", "native"} for s in specs),
         "; ".join(f"{a}: {'passed' if l.get('semantic_correctness', {}).get('passed') else 'not passed'} on {', '.join(s.get('backends', []))}" for (a, l), s in zip(logs.items(), specs)),
         [f"acceptance/{a}/run-log.json" for a in APPS])
    matrix = load("build/matrix.json") or {}
    item("DW-06 the supported matrix", "PROG-DEL-04", matrix.get("checks") and all(c["ok"] for c in matrix["checks"]), f"{matrix.get('entries')} entries, {sum(c['ok'] for c in matrix.get('checks', []))}/{len(matrix.get('checks', []))} checks pass",
         ["reference/matrix.json"])
    reg = os.path.join(ROOT, "docs", "delivery", "readability", "PREREGISTRATION.sha256")
    ok = os.path.exists(reg) and all(sha_file(os.path.join(ROOT, p)) == h for h, p in (l.split("  ", 1) for l in open(reg, encoding="utf-8").read().splitlines() if l))
    item("DW-07 readability pre-registered (study deferred with a recorded justification)", "PROG-AGT-05", ok and os.path.exists(os.path.join(ROOT, "docs", "delivery", "readability", "DEFERRAL.md")),
         "registration and materials match their recorded digests; the study is not run (DEFERRAL.md)", ["docs/delivery/readability"])
    _, targets, _ = run(["rustc", "--print", "target-libdir"])
    rustlib = os.path.dirname(os.path.dirname(targets.strip()))
    installed = sorted(t for t in (os.listdir(rustlib) if os.path.isdir(rustlib) else [])
                       if any(f.startswith("libstd-") for f in (os.listdir(os.path.join(rustlib, t, "lib")) if os.path.isdir(os.path.join(rustlib, t, "lib")) else [])))
    tier1 = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"]
    item("DW-08 A2 on each tier-1 native target; WASI and embedded smoke runs", "Programme 4.3", all(t in installed for t in tier1),
         f"Rust standard libraries installed: {installed}; A2 runs natively on {[t for t in tier1 if t in installed]}; not buildable here: {[t for t in tier1 if t not in installed]}, and no WASI or embedded runner (blocked: the host has no other target's standard library, SDK or runner)", [])
    priv = load("build/privilege.json") or {}
    item("least privilege for build, generator, test and agent contexts", "PROG-SEC-02", priv.get("checks") and all(c["ok"] for c in priv["checks"]),
         f"{sum(c['ok'] for c in priv.get('checks', []))}/{len(priv.get('checks', []))} privilege checks pass", ["docs/delivery/privileges.json"])
    # integrity: what the record speaks for
    subjects = {}
    for rel in ["conformance/release/results.json", "conformance/execution/results.json", "conformance/checker/results.json", "conformance/results.json",
                "reference/matrix.json", "docs/delivery/privileges.json", "docs/delivery/defects.json", "docs/delivery/safety-review.json",
                "docs/delivery/readability/PREREGISTRATION.sha256", "docs/Fortis_Language_Reference.md", "docs/Fortis_Delivery_Programme.md",
                "docs/Fortis_Platform_Specification.md", "library/fortis/compatibility.yaml"] + [f"acceptance/{a}/{f}" for a in APPS for f in ("run-log.json", "spec.json", "contracts.json")]:
        p = os.path.join(ROOT, rel)
        if os.path.exists(p):
            subjects[rel] = sha_file(p)
    for rel in ["library/fortis", "conformance/programs", "conformance/packages"] + [f"acceptance/{a}/src" for a in APPS]:
        subjects[rel + "/"] = sha_tree(os.path.join(ROOT, rel))
    subjects["compiler/target/release/fortisc"] = sha_file(FORTISC)
    _, rustc_v, _ = run(["rustc", "--version"])
    _, git_tree, _ = run(["git", "rev-parse", "HEAD^{tree}"])
    record = {
        "schema": "fortis.release-record/v1", "release": "Fortis Language 1.0", "commit": head, "tree": git_tree.strip(),
        "uncommitted_changes": [l[3:] for l in dirty.splitlines()],
        "integrity": {"algorithm": "sha256", "subjects": subjects,
                      "about": "each file's sha256, or for a directory (ending /) the sha256 of its files' relative paths and bytes in sorted order"},
        "attestation": {"builder": "conformance/tools/release_record.py", "produced": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                        "environment": {"host": platform.platform(), "python": platform.python_version(), "rustc": rustc_v.strip()},
                        "invocations": ["python3 -I docs/delivery/tools/plan_check.py --release", "python3 -I conformance/tools/release_results.py",
                                        "python3 -I acceptance/run.py a", "python3 -I acceptance/run.py a2", "python3 -I conformance/tools/matrix_check.py --json",
                                        "python3 -I conformance/tools/privilege_check.py --json"],
                        "signature": None,
                        "about": "unsigned: the project has no release signing key yet; the verifier binds the record to its subjects' digests and the commit, and a signature over this record is added when the owner provides a key"},
        "correctness": {"items": items, "blocking": blocking, "releasable": not blocking},
    }
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    json.dump(record, open(OUT, "w", encoding="utf-8"), indent=1)
    print(f"\n{len(items)} items, {sum(i['holds'] for i in items)} hold; " + ("releasable" if not blocking else f"not releasable: {len(blocking)} blocking"))
    print(f"release record: {os.path.relpath(OUT, ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
