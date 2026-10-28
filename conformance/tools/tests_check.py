"""DEL-43 criterion 2 (runner part): one runner for every testing mode of Language 1.0 (PROG-TST-01), and generated,
table and fuzz failures reproducible from a seed and minimised (PROG-TST-02); X-131.

`python3 -I conformance/tools/tests_check.py [--json]`

The fixture `conformance/testing/modes.ft` holds every mode with planted failures whose minimal inputs are known:
example tests (one passing, one failing), a table test with a failing row, a passing property, a failing property
(minimal `[-1]`), a property whose failure random generation misses but coverage-guided fuzzing reaches (minimal
`[1, 2, 3]`), and a property that hangs on negative inputs (minimal `-1`). `fortisc test` runs them all:

1. **One runner**: every mode yields its records from the one command; the reference application
   `examples/apps/orders` exercises example, table and property tests through it too.
2. **Reproducible**: two runs give identical records; the replay command a `property-failure` record names
   reproduces the failure and the same minimal input; a table row reruns alone by its name; the same replay in a
   fresh copy of the module (a clean checkout) gives the same records; a fuzz run is deterministic from its seed, and
   its saved failure replays.
3. **Minimised**: each minimal input is the known one, and it fails when replayed alone.
4. **Fuzzing**: random generation (100 cases) misses the rare failure; fuzzing finds it; the corpus persists and is
   minimised; a hang is a finding.

The package fixture `conformance/testing/acme.modtest` covers the test runner on one module of a package (D-54
item 9): its module `calc` tests functions its sibling `numbers` exports. `fortis test FILE.ft` (the interpreter),
`fortisc test FILE.ft` (the VM) and `fortisc native FILE.ft --tests` each find the enclosing package, load it, and
run that module's tests with its imports resolved, all three agreeing; only the named module's tests run; the
package directory still runs every module's tests; a file of the package's directory outside its src/ stays
standalone; and a symlink to a module's file resolves to it and runs as the package's module. `calc` also carries
an observation declared by `numbers`, so the plan step resolves that schema in the declaring module.
`conformance/testing/acme.obsstr` is the neighbour that must still be refused (an imported alias of String).
`conformance/testing/acme.dotname` holds a module file whose name begins with `..`.
"""
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
sys.path.insert(0, ROOT)
from interpreter import records as runtime  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
FORTIS = os.path.join(ROOT, "bin", "fortis")
MODES = "conformance/testing/modes.ft"
PKG = "conformance/testing/acme.modtest"
OBS = "conformance/testing/acme.obsstr"
DOT = "conformance/testing/acme.dotname"


def test(*args, cwd=ROOT):
    r = subprocess.run([FORTISC, "test", "--format", "json"] + list(args), capture_output=True, text=True, cwd=cwd, timeout=1800)
    recs = [json.loads(l) for l in r.stdout.splitlines() if l.startswith("{")]
    return r.returncode, recs


def fortis_test(*args):
    """`fortis test`: the summary on standard output, every record on standard error."""
    r = subprocess.run([sys.executable, "-I", FORTIS, "test", "--format", "json"] + list(args), capture_output=True, text=True, cwd=ROOT, timeout=1800)
    out = [json.loads(l) for l in r.stdout.splitlines() if l.strip().startswith("{")]
    err = [json.loads(l) for l in r.stderr.splitlines() if l.strip().startswith("{")]
    return r.returncode, out, err


def native_tests(*args):
    """`fortisc native --tests`: the observed line and the records of the lowered crate's run."""
    r = subprocess.run([FORTISC, "native", *args, "--tests"], capture_output=True, text=True, cwd=ROOT, timeout=1800)
    obs = [l[len("observed: "):] for l in r.stdout.splitlines() if l.startswith("observed: ")]
    recs = [json.loads(l[len("record: "):]) for l in r.stdout.splitlines() if l.startswith("record: ") and l[len("record: "):].strip().startswith("{")]
    return r.returncode, obs, recs


def statuses(recs):
    """{(test, case): status} of a run's test results."""
    return {(r["test"], r["case"]): r["status"] for r in recs if r.get("kind") == "test-result"}


def summary(out):
    s = [r for r in out if r.get("kind") == "test-summary"]
    return (s[0].get("passed"), s[0].get("failed")) if len(s) == 1 else None


def results(recs, kind="test-result"):
    return [r for r in recs if r.get("kind") == kind]


def replay_args(cmd):
    """The arguments after `fortis test FILE` in a record's replay command."""
    return shlex.split(cmd)[3:]


def main(argv):
    rows, problems = [], 0

    def expect(name, ok, detail=""):
        nonlocal problems
        rows.append({"check": name, "ok": bool(ok), "detail": str(detail)[:600]})
        problems += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok else f": {str(detail)[:400]}"))

    code, recs = test(MODES)
    res = {(r["test"], r["case"]): r for r in results(recs)}
    fails = {r["test"]: r for r in results(recs, "property-failure")}
    try:
        for r in recs:
            runtime.validate(r)
        valid = True
    except runtime.RecordError as e:
        valid = str(e)
    expect("every record is valid against the runtime schema", valid is True, valid)
    # 1. every mode on one runner
    status = lambda t, c: res.get((t, c), {}).get("status")  # noqa: E731
    expect("example tests: one passes, one fails", status("an example passes", "") == "pass" and status("an example fails", "") == "fail", res.keys())
    expect("table test: each row is a case; the planted row fails", [status("clamping by table", c) for c in ("under", "over", "wrong")] == ["pass", "pass", "fail"])
    expect("property: 100 generated cases pass", status("a clamped sum never exceeds the limit", "100 cases") == "pass")
    expect("property: the planted failure is found and minimised to [-1]", fails.get("the maximum is an element", {}).get("minimal") == {"xs": [-1]}, fails.get("the maximum is an element"))
    expect("property: random generation misses the rare failure", status("no run of 1, 2, 3", "100 cases") == "pass")
    hang = fails.get("countdown terminates", {})
    expect("a hang is a finding (the execution budget), minimised to -1", hang.get("minimal") == {"n": -1} and "hang" in hang.get("evidence", {}).get("actual", ""), hang)
    expect("the run fails (exit 1) and ends with a summary", code == 1 and recs and recs[-1].get("kind") == "test-summary")
    # 2. reproducible
    _, again = test(MODES)
    expect("two runs give identical records", again == recs)
    f = fails.get("the maximum is an element")
    if f:
        _, rep = test(MODES, *replay_args(f["replay"]))
        rf = results(rep, "property-failure")
        expect("the replay command reproduces the failing case and the same minimal input", rf and rf[0]["minimal"] == f["minimal"] and rf[0]["case"] == f["case"] and rf[0]["seed"] == f["seed"], rep)
        clean = tempfile.mkdtemp(prefix="fortis-replay-")
        try:
            shutil.copy(os.path.join(ROOT, MODES), clean)
            _, rep2 = test("modes.ft", *replay_args(f["replay"]), cwd=clean)
            strip = lambda rs: [{k: v for k, v in r.items() if k != "replay"} for r in rs]  # noqa: E731
            expect("the same replay in a fresh copy (a clean checkout) gives the same records", strip(rep2) == strip(rep), rep2)
        finally:
            shutil.rmtree(clean, ignore_errors=True)
    _, row = test(MODES, "--test", "clamping by table", "--case", "wrong")
    expect("a failing table row reruns alone by its name, with the same evidence", [(r["case"], r["status"], r["evidence"]) for r in results(row)] == [("wrong", "fail", res[("clamping by table", "wrong")]["evidence"])])
    # 4. fuzzing, with a corpus directory per run
    work = tempfile.mkdtemp(prefix="fortis-fuzz-")
    try:
        corpus = os.path.join(work, "corpus")
        _, fz = test(MODES, "--test", "no run of 1, 2, 3", "--fuzz", "--budget", "20000", "--corpus", corpus)
        ff = results(fz, "property-failure")
        expect("fuzzing reaches the rare failure and minimises it to [1, 2, 3]", ff and ff[0]["mode"] == "fuzz" and ff[0]["minimal"] == {"xs": [1, 2, 3]}, fz)
        corpus2 = os.path.join(work, "corpus2")
        _, fz2 = test(MODES, "--test", "no run of 1, 2, 3", "--fuzz", "--budget", "20000", "--corpus", corpus2)
        strip = lambda rs: [{k: v for k, v in r.items() if k != "replay"} for r in rs]  # noqa: E731
        expect("a fuzz run is deterministic from its seed", strip(fz2) == strip(fz))
        if ff:
            _, rp = test(MODES, *replay_args(ff[0]["replay"]))
            expect("the saved fuzz failure replays and fails", [(r["case"], r["status"]) for r in results(rp)] == [("replay", "fail")], rp)
        _, ok1 = test(MODES, "--test", "a clamped sum never exceeds the limit", "--fuzz", "--budget", "3000", "--corpus", corpus)
        target = [d for d in os.listdir(corpus) if d.startswith("a-clamped-sum")]
        entries = sorted(os.listdir(os.path.join(corpus, target[0]))) if target else []
        first = results(ok1)[0] if results(ok1) else {}
        _, ok2 = test(MODES, "--test", "a clamped sum never exceeds the limit", "--fuzz", "--budget", "3000", "--corpus", corpus)
        second = results(ok2)[0] if results(ok2) else {}
        expect(f"a passing target's corpus persists ({len(entries)} entries) and stays minimised across runs",
               first.get("status") == "pass" and entries and second.get("evidence", {}).get("branches", 0) >= first.get("evidence", {}).get("branches", 0)
               and second.get("evidence", {}).get("corpus", 0) <= len(entries) + 1, (first, second, entries))
    finally:
        shutil.rmtree(work, ignore_errors=True)
    # 1 (reference application)
    _, app = test("examples/apps/orders")
    kinds = {("table" if r["case"] and not r["case"].endswith("cases") else "property" if r["case"].endswith("cases") else "example") for r in results(app)}
    expect("the reference application exercises example, table and property tests on the runner, all passing",
           kinds == {"example", "table", "property"} and all(r["status"] == "pass" for r in results(app)), [(r["test"], r["case"], r["status"]) for r in results(app)])
    # 5. one module of a package (D-54 item 9): every test runner finds the enclosing package of a module file,
    #    loads it, and runs that module's tests with its imports resolved; the three backends agree
    calc = os.path.join(PKG, "src", "calc.ft")
    want = {("a test calls the sibling's functions", ""): "pass",
            ("a table test calls the sibling's functions", "two"): "pass",
            ("a table test calls the sibling's functions", "three"): "pass"}
    icode, isum, ires = fortis_test(calc)
    interp = statuses(ires)
    expect("the interpreter runs a module's tests with its package imports resolved, and only that module's",
           icode == 0 and interp == want and summary(isum) == (3, 0), (icode, interp, isum))
    vcode, vrecs = test(calc)
    vm = statuses(vrecs)
    expect("the VM runs the same module's tests with its package imports resolved, and only that module's",
           vcode == 0 and vm == want and summary(vrecs) == (3, 0), (vcode, vm))
    ncode, obs, nres = native_tests(calc)
    nat = statuses(nres)
    expect("native --tests runs the same module's tests with its package imports resolved, and all three agree",
           ncode == 0 and obs == ["3 passed, 0 failed"] and nat == want, (ncode, obs, nat))
    alone = []
    for f, one in (("numbers.ft", {("the sibling's own test", ""): "pass"}),
                   ("app.ft", {("the entry's test calls the module under test", ""): "pass"}),
                   ("util/extra.ft", {("a nested module calls the sibling", ""): "pass"}),
                   ("carry.ft", {("an imported observation of an alias of Int runs", ""): "pass"})):
        p = os.path.join(PKG, "src", f)
        vcode, vrecs = test(p)
        icode, isum, ires = fortis_test(p)
        alone.append((f, vcode, statuses(vrecs), summary(vrecs), icode, statuses(ires), summary(isum), one))
    expect("each other module's file runs its own tests alone, on both runners",
           all(vc == 0 and vs == one and vsum == (1, 0) and ic == 0 and ir == one and isum == (1, 0)
               for (_, vc, vs, vsum, ic, ir, isum, one) in alone), alone)
    every = {**want, ("the sibling's own test", ""): "pass", ("the entry's test calls the module under test", ""): "pass",
             ("a nested module calls the sibling", ""): "pass",
             ("an imported observation of an alias of Int runs", ""): "pass"}
    dcode, drecs = test(PKG)
    dcode2, dsum2, dres2 = fortis_test(PKG)
    expect("the package directory still runs every module's tests, on both runners",
           dcode == 0 and statuses(drecs) == every and summary(drecs) == (7, 0)
           and dcode2 == 0 and statuses(dres2) == every and summary(dsum2) == (7, 0),
           (dcode, statuses(drecs), dcode2, statuses(dres2)))
    tool = os.path.join(PKG, "tool.ft")
    tcode, tsum, tres = fortis_test(tool)
    t2code, trecs = test(tool)
    expect("a file of the package's directory outside its src/ stays standalone, on both runners",
           tcode == 0 and statuses(tres) == {("a standalone file's test", ""): "pass"} and summary(tsum) == (1, 0)
           and t2code == 0 and statuses(trecs) == {("a standalone file's test", ""): "pass"} and summary(trecs) == (1, 0),
           (tcode, statuses(tres), t2code, statuses(trecs)))
    link = os.path.join(ROOT, "build", "modtest-symlink.ft")
    os.makedirs(os.path.dirname(link), exist_ok=True)
    if os.path.lexists(link):
        os.remove(link)
    os.symlink(os.path.join(ROOT, PKG, "src", "calc.ft"), link)
    lcode, lsum, lres = fortis_test(link)
    l2code, l2recs = test(link)
    expect("a symlink to a module's file runs as the package's module, on both runners",
           lcode == 0 and statuses(lres) == want and summary(lsum) == (3, 0)
           and l2code == 0 and statuses(l2recs) == want and summary(l2recs) == (3, 0),
           (lcode, statuses(lres), l2code, statuses(l2recs)))
    # an imported alias of String is still allocating, on both runners, at the same policy site
    app = os.path.join(OBS, "src", "app.ft")
    ocode, osum, oerr = fortis_test(app)
    orecs = [r for r in osum if r.get("code") == "E-EFF-030"]
    vcode, vrecs = test(app)
    vhits = [r for r in vrecs if r.get("code") == "E-EFF-030"]
    ncode, nobs, _ = native_tests(app)
    same = (ocode == 1 and vcode == 1 and len(orecs) == 1 and len(vhits) == 1
            and (orecs[0]["line"], orecs[0]["column"]) == (vhits[0]["line"], vhits[0]["column"])
            and "label: Label" in orecs[0]["message"] and "label: Label" in vhits[0]["message"]
            and ncode == 0 and nobs == ["refused:E-EFF-030"])
    expect("an imported alias of String is still E-EFF-030, on both runners and native, at the same site",
           same, (ocode, orecs, vcode, vhits, ncode, nobs))
    dot = os.path.join(DOT, "src", "..calc.ft")
    d1, d1sum, d1res = fortis_test(dot)
    d2, d2recs = test(dot)
    one = {("a file named with two leading dots imports its sibling", ""): "pass"}
    expect("a file whose name begins with .. is a module of the package, on both runners",
           d1 == 0 and statuses(d1res) == one and summary(d1sum) == (1, 0)
           and d2 == 0 and statuses(d2recs) == one and summary(d2recs) == (1, 0),
           (d1, statuses(d1res), d2, statuses(d2recs)))
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rules": ["PROG-TST-01", "PROG-TST-02"], "checks": rows, "records": recs}, open(os.path.join(ROOT, "build", "tests.json"), "w"), indent=1)
    print(f"\n{len(rows)} checks, {sum(r['ok'] for r in rows)} pass")
    print("tests check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
