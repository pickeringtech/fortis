"""DEL-43 criterion 6: E-LIM and E-INT records, backend errors mapped to source, and a quota on every compiler input
parser (PROG-CMP-05, PROG-SEC-01).

`python3 -I conformance/tools/limits_check.py [--json]`

1. **E-LIM, instantiation**: `conformance/limits/instantiation.ft` instantiates a generic function at ever larger
   types; it checks and runs on the VM, and `fortisc build` stops with `E-LIM-001` naming the budget and the
   alternative before lowering (no crate is generated).
2. **E-INT, injected fault**: with `FORTIS_FAULT=typer:function total`, checking `conformance/limits/fault.ft` fails
   inside the checker; the command does not crash but reports `E-INT-001` whose reproduction is a directory holding
   a minimised input (smaller than the original) and the command; running that command on that input fails again
   the same way.
3. **Backend error to source** (PROG-CMP-05): with `FORTIS_FAULT=lower:function process` the generated Rust does not
   compile; `fortisc build` reports `E-INT-002` at the Fortis file and line of `process`, with a reproduction.
4. **Quotas** (PROG-SEC-01): one quota-exceeded fixture per compiler input parser, each generated here, each
   refused with its `E-LIM` record (schema-valid) and never a crash: source size, tokens, bracket nesting, a run of
   prefix operators, string-interpolation nesting (E-LIM-002), a manifest (E-LIM-003), a lockfile (E-LIM-004),
   JSON nesting in a saved fuzz input (E-LIM-005), and inputs within each quota still accepted. The inventory of
   parsers names the formats Language 1.0 does not have.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import diagnostics  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
LIMITS = os.path.join(ROOT, "conformance", "limits")

INVENTORY = [
    ("Fortis source (lexer and parser)", "16 MiB, 2,000,000 tokens, nesting 256 (brackets, prefix-operator runs, interpolation)", "E-LIM-002"),
    ("project.yaml (manifest reader)", "1 MiB, 10,000 lines", "E-LIM-003"),
    ("fortis.lock (lockfile parser)", "8 MiB, 100,000 lines", "E-LIM-004"),
    ("JSON (query-database caches, saved fuzz inputs, cargo metadata)", "256 MiB, nesting 128; a saved input 1 MiB", "E-LIM-005"),
    ("library/fortis/compatibility.yaml", "1 MiB (an oversized set admits no standard package)", "E-NAM-031"),
    ("bytecode", "none in Language 1.0: the VM consumes typed IR built in process, never a file", "—"),
    ("package archives", "none in Language 1.0: packages are directories (Platform: kit Packages)", "—"),
    ("telemetry plans", "the Telemetry kit's input (its group)", "—"),
]


def fortisc(args, cwd=ROOT, env=None):
    r = subprocess.run([FORTISC] + args, capture_output=True, text=True, cwd=cwd, timeout=1800, env=env)
    recs = [json.loads(l) for l in r.stdout.splitlines() if l.startswith("{") and '"code"' in l]
    return r.returncode, recs, r.stdout + r.stderr


def main(argv):
    rows, problems = [], 0

    def expect(name, ok, detail=""):
        nonlocal problems
        rows.append({"check": name, "ok": bool(ok), "detail": str(detail)[:700]})
        problems += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok else f": {str(detail)[:400]}"))

    def valid(recs):
        return all(not diagnostics.validate(r) for r in recs)

    # 1. instantiation budget
    inst = os.path.join(LIMITS, "instantiation.ft")
    code, recs, out = fortisc(["check", "--format", "json", inst])
    vm = subprocess.run([FORTISC, "vm", inst], capture_output=True, text=True, cwd=ROOT).stdout
    gen = os.path.join(ROOT, "build", "native-build", "fb_instantiation")
    shutil.rmtree(gen, ignore_errors=True)
    bcode, brecs, bout = fortisc(["build", "--format", "json", inst])
    lim = [r for r in brecs if r["code"] == "E-LIM-001"]
    expect("a generic function instantiating itself at ever larger types checks and runs on the VM", code == 0 and "observed: " in vm and "dynamic:" not in vm, out + vm)
    expect("its build stops with E-LIM-001 naming the budget, the chain and the alternative, before lowering",
           bcode == 1 and len(lim) == 1 and "instantiation depth" in lim[0]["fields"]["budget"] and "non-generic helper" in lim[0]["message"] and valid(lim) and not os.path.exists(gen), bout[-500:])
    # 2. injected internal fault, minimised
    fault = os.path.join(LIMITS, "fault.ft")
    env = dict(os.environ, FORTIS_FAULT="typer:function total")
    code, recs, out = fortisc(["check", "--format", "json", fault], env=env)
    intl = [r for r in recs if r["code"] == "E-INT-001"]
    repro = intl[0]["fields"]["reproduction"] if intl else ""
    minimal = open(os.path.join(repro, "input.ft"), encoding="utf-8").read() if repro and os.path.exists(os.path.join(repro, "input.ft")) else ""
    original = open(fault, encoding="utf-8").read()
    expect("an injected checker fault is an E-INT-001 record, not a crash", code == 1 and len(intl) == 1 and valid(intl), out[-400:])
    expect(f"its reproduction is minimised ({len(minimal.splitlines())} of {len(original.splitlines())} lines) and saved with its command",
           minimal and len(minimal.splitlines()) < len(original.splitlines()) and os.path.exists(os.path.join(repro, "README")), repro)
    if minimal:
        c2, r2, o2 = fortisc(["check", "--format", "json", os.path.join(repro, "input.ft")], env=env)
        expect("the minimised input reproduces the same internal fault", c2 == 1 and any(r["code"] == "E-INT-001" for r in r2), o2[-300:])
        c3, r3, _ = fortisc(["check", "--format", "json", fault])
        expect("without the fault the module checks", c3 == 0 and not r3)
    # 3. backend error mapped to source
    env = dict(os.environ, FORTIS_FAULT="lower:function process")
    code, recs, out = fortisc(["build", "--format", "json", "examples/apps/orders"], env=env)
    be = [r for r in recs if r["code"] == "E-INT-002"]
    line = next(i for i, l in enumerate(open(os.path.join(ROOT, "examples/apps/orders/src/orders.ft"), encoding="utf-8"), start=1) if "func process(" in l)
    expect(f"a backend error maps to the Fortis source (orders.ft:{line}) and is classified as a compiler defect (E-INT-002)",
           code == 1 and len(be) == 1 and be[0]["file"].endswith("examples/apps/orders/src/orders.ft") and be[0]["line"] == line and be[0]["class"] == "Internal defect" and "rustc" in be[0]["fields"]["reproduction"] and valid(be), out[-400:])
    # 4. quotas
    work = tempfile.mkdtemp(prefix="fortis-limits-")
    try:
        def source_case(name, text, budget_word):
            p = os.path.join(work, name)
            open(p, "w").write(text)
            code, recs, out = fortisc(["check", "--format", "json", p])
            ok = code == 1 and len(recs) == 1 and recs[0]["code"] == "E-LIM-002" and budget_word in recs[0]["fields"]["budget"] and valid(recs)
            expect(f"source quota: {name} is refused with E-LIM-002 ({recs[0]['fields']['operation'] if recs else 'no record'})", ok, out[-300:])
            return p

        deep = source_case("nesting.ft", "func f() Int {\n  return " + "(" * 20000 + "1" + ")" * 20000 + "\n}\n", "nesting")
        source_case("prefix.ft", "func f() Bool {\n  return " + "!" * 5000 + "true\n}\n", "nesting")
        source_case("interpolation.ft", "func f() String {\n  return " + '"${' * 2000 + "1" + '}"' * 2000 + "\n}\n", "nesting")
        source_case("tokens.ft", "func f() Int {\n  return " + "1 + " * 1_000_001 + "1\n}\n", "tokens")
        source_case("size.ft", "// " + "x" * (17 << 20) + "\nfunc f() Int => 1\n", "bytes")
        within = os.path.join(work, "within.ft")
        open(within, "w").write("func f() Int {\n  return " + "(" * 200 + "1" + ")" * 200 + "\n}\n")
        code, recs, out = fortisc(["check", "--format", "json", within])
        expect("a source within the quotas (nesting 200) checks", code == 0 and not recs, out[-300:])
        # every tool refuses the over-quota source without crashing
        crashes = []
        for args in (["fmt", deep], ["view", deep], ["test", "--format", "json", deep], ["build", "--format", "json", deep], ["syntax", deep], ["parse", deep]):
            r = subprocess.run([FORTISC] + args, capture_output=True, text=True, cwd=ROOT, timeout=600)
            if r.returncode < 0 or r.returncode in (101, 134) or "overflowed its stack" in r.stderr:
                crashes.append((args[0], r.returncode, r.stderr[-120:]))
        r = subprocess.run([os.path.join(ROOT, "compiler", "target", "release", "fortis-query-db"), "records", deep], capture_output=True, text=True, timeout=600)
        if r.returncode < 0 or "overflowed" in r.stderr:
            crashes.append(("query-db", r.returncode, r.stderr[-120:]))
        expect("fmt, view, test, build, syntax, parse and the query database refuse the over-quota source without crashing", not crashes, crashes)
        # manifest
        pkg = os.path.join(work, "big.manifest")
        os.makedirs(os.path.join(pkg, "src"))
        open(os.path.join(pkg, "project.yaml"), "w").write("schema: fortis.project/v1\nname: big.manifest\nversion: 1.0.0\nexports: [app]\n" + "".join(f"# filler {i}\n" for i in range(12000)) + "dependencies:\n  fortis: \"1.0\"\n")
        open(os.path.join(pkg, "src", "app.ft"), "w").write("export pure func answer() Int => 42\n")
        code, recs, out = fortisc(["check", "--format", "json", pkg])
        expect("manifest quota: a manifest of 12,000 lines is refused with E-LIM-003", code == 1 and [r["code"] for r in recs] == ["E-LIM-003"] and valid(recs), out[-300:])
        # lockfile
        lpkg = os.path.join(work, "big.lock")
        os.makedirs(os.path.join(lpkg, "src"))
        open(os.path.join(lpkg, "project.yaml"), "w").write("schema: fortis.project/v1\nname: big.lock\nversion: 1.0.0\nexports: [app]\ndependencies:\n  fortis: \"1.0\"\n")
        open(os.path.join(lpkg, "src", "app.ft"), "w").write("export pure func answer() Int => 42\n")
        open(os.path.join(lpkg, "fortis.lock"), "w").write("schema: fortis.lock/v1\n" + "".join(f"# filler {i}\n" for i in range(120000)))
        code, recs, out = fortisc(["check", "--format", "json", lpkg])
        expect("lockfile quota: a lockfile of 120,000 lines is refused with E-LIM-004", code == 1 and [r["code"] for r in recs] == ["E-LIM-004"] and valid(recs), out[-300:])
        # JSON nesting in a saved fuzz input
        saved = os.path.join(work, "deep.json")
        open(saved, "w").write('{"test": "x", "inputs": ' + "[" * 100000 + "]" * 100000 + "}")
        r = subprocess.run([FORTISC, "test", "--format", "json", "conformance/testing/modes.ft", "--replay", saved], capture_output=True, text=True, cwd=ROOT, timeout=600)
        res = [json.loads(l) for l in r.stdout.splitlines() if '"test-result"' in l]
        expect("JSON quota: a saved input nested 100,000 deep is refused with E-LIM-005, without a crash",
               r.returncode == 1 and res and "E-LIM-005" in res[0]["evidence"]["actual"] and "overflowed" not in r.stderr, (r.stdout + r.stderr)[-300:])
    finally:
        shutil.rmtree(work, ignore_errors=True)
    for parser, quota, code_ in INVENTORY:
        print(f"     parser: {parser}: {quota} ({code_})")
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rules": ["PROG-CMP-05", "PROG-SEC-01"], "codes": ["E-LIM-001", "E-LIM-002", "E-LIM-003", "E-LIM-004", "E-LIM-005", "E-INT-001", "E-INT-002"],
                   "parsers": [{"parser": p, "quota": q, "code": c} for p, q, c in INVENTORY], "checks": rows}, open(os.path.join(ROOT, "build", "limits.json"), "w"), indent=1)
    print(f"\n{len(rows)} checks, {sum(r['ok'] for r in rows)} pass")
    print("limits check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
