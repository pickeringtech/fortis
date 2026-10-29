"""DEL-08 runner: executes every `// execute:` fixture tagged `// requires: runtime` on the
definitional interpreter and compares its console output (lines joined by `|`) or its failure
(`fail:<code>`) with the marker. Writes conformance/execution/results.json (backend `interpreter`).
A `// reject:` fixture tagged `requires: runtime` is refused by the harness (the kernel makes no
static claims). `--negative` runs that refusal test. Every run also checks the interpreter's backstop (D-110): programs
the checker refuses for creating the capabilities twice, run without their checks, must stop with the
runtime-invariant defect (10.8), never with a second set of capabilities."""
import hashlib
import json
import os
import re
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
sys.path.insert(0, ROOT)
from interpreter.eval import Interpreter  # noqa: E402
from interpreter.values import FortisError
from interpreter.executor import DefectSignal  # noqa: E402

PROGRAMS = os.path.join(ROOT, "conformance", "programs")
MARK = re.compile(r"^//\s*execute:\s*(.*)$", re.M)
REQ = re.compile(r"^//\s*requires:\s*(.*)$", re.M)


_LIB = None


def static_records(path, src):
    """The interpreter never evaluates a program the checker rejects (DEL-21 criterion 2)."""
    global _LIB
    import lexer, names, parser  # noqa: E401
    from checker import check
    if _LIB is None:
        _LIB = names.Library(parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf")))
    try:
        tree = _LIB.rec.parse(lexer.lex(src), "module")
    except Exception as e:  # noqa: BLE001
        return [{"code": "E-SYN-000", "message": str(e)}]
    return [r for r in check.check_tree(_LIB, tree, os.path.relpath(path, ROOT)) if r.get("code")]


TESTS = re.compile(r"^//\s*tests:\s*(.*)$", re.M)
TRACE = re.compile(r"^//\s*execute-trace:\s*(.*)$", re.M)
REPORTS_LAST = re.compile(r"^//\s*reports-last:\s*(.*)$", re.M)
REPORTS = re.compile(r"^//\s*reports:\s*(.*)$", re.M)


def reports_match(want: str, reports) -> bool:
    """`// reports: kind:code|kind:field=value,...` lists, in order, the records the run must emit (no others)."""
    want_items = [w.strip() for w in want.split("|") if w.strip()]
    if len(want_items) != len(reports):
        return False
    for w, r in zip(want_items, reports):
        kind, _, detail = w.partition(":")
        if r["kind"] != kind:
            return False
        if "=" in detail:
            fields = r.get("fields", {})
            for kv in detail.split(","):
                k, _, v = kv.partition("=")
                if str(r.get(k, fields.get(k))) != v:
                    return False
        elif detail and r.get("code", r.get("status", "")) != detail:
            return False
    return True


def command_line_output(path):
    """(console lines, exit status) of `fortis run` on a program that ends in a defect: the lines printed before
    the defect stay printed and the status is 3 (10.8; the command once lost the lines, D-53)."""
    r = subprocess.run([sys.executable, "-I", os.path.join(ROOT, "bin", "fortis"), "run", "--no-check", path],
                       capture_output=True, text=True, cwd=ROOT, timeout=300)
    return r.stdout.splitlines(), r.returncode


def run_fixture(path):
    src = open(path, encoding="utf-8").read()
    rejected = static_records(path, src)
    if rejected:
        return "refused:" + ",".join(sorted({r["code"] for r in rejected})), None
    it = Interpreter(src, os.path.relpath(path, ROOT))
    tm = TESTS.search(src)
    try:
        if tm is not None:
            passed, failed = it.run_tests()
            observed = f"{passed} passed, {failed} failed"
        else:
            code = it.run_main([])
            observed = "|".join(it.out + (["fail:" + it.reports[-1]["code"]] if code != 0 else []))
    except FortisError as e:
        return f"dynamic:{e}", it
    except RecursionError:
        observed = "|".join(it.out + ["defect:stack_exhaustion"])          # a defect terminates the program and is never caught (10.8)
        cli = command_line_output(path)
        return (observed if cli == (it.out, 3) else observed + f" with `fortis run` giving {cli!r}"), it
    except DefectSignal as d:
        observed = "|".join(it.out + [f"defect:{d.kind}"])
        cli = command_line_output(path)
        if cli != (it.out, 3):
            return observed + f" with `fortis run` giving {cli!r}", it
        rl = REPORTS_LAST.search(src)
        if rl is not None and not reports_match(rl.group(1), it.reports[-1:]):
            return observed + " with last report " + json.dumps(it.reports[-1]), it
        return observed, it
    rl = REPORTS_LAST.search(src)
    if rl is not None and not reports_match(rl.group(1), it.reports[-1:]):
        return observed + " with last report " + json.dumps(it.reports[-1] if it.reports else {}), it
    rm = REPORTS.search(src)
    if rm is not None and not reports_match(rm.group(1), it.reports):
        return observed + " with reports " + "|".join(f"{r['kind']}:{r.get('code', r.get('status', ''))}" for r in it.reports), it
    tr = TRACE.search(src)
    if tr is not None and tr.group(1).strip() != "|".join(it.trace):
        return observed + " with trace " + "|".join(it.trace), it
    return observed, it


# (fixture, expected observation): run unchecked, as a checker defect would let them run
BACKSTOPS = [("auth_caps_twice.ft", "defect:runtime_invariant")]


def backstop_test() -> int:
    """The interpreter refuses a second creation of the capabilities (11.6) by itself (D-110)."""
    failures = 0
    for name, want in BACKSTOPS:
        path = os.path.join(ROOT, "conformance", "programs", name)
        it = Interpreter(open(path, encoding="utf-8").read(), os.path.relpath(path, ROOT))
        sys.stderr = open(os.devnull, "w")
        try:
            it.run_main([])
            observed = "|".join(it.out + ["ran to completion"])
        except DefectSignal as d:
            observed = "|".join(it.out + [f"defect:{d.kind}"])
        except FortisError as e:
            observed = f"dynamic:{e}"
        finally:
            sys.stderr = sys.__stderr__
        ok = observed == want
        failures += not ok
        print(f"{'PASS' if ok else 'FAIL'} backstop {name:<27} {'' if ok else observed}")
    return failures + cleanup_backstop_test()


# programs a checker defect let run, built whatever the checker says; fortisc's tests/unchecked_cleanup.rs runs the
# same programs on the VM
UNCHECKED = os.path.join(ROOT, "compiler", "crates", "fortisc", "tests", "unchecked")


def cleanup_backstop_test() -> int:
    """The interpreter does not rely on the checker for a class value's cleanup to run exactly once (LR 7.9, 12.2;
    D-348): a loan that outlives its owner's destruction, or two bindings that own one value, still print the
    program's `// backstop:` lines; a program that reads what a `return` moved out stops with a dynamic check
    failure, the last item, never silently (D-391)."""
    failures = 0
    for name in sorted(os.listdir(UNCHECKED)):
        if not name.endswith(".ft"):
            continue
        path = os.path.join(UNCHECKED, name)
        src = open(path, encoding="utf-8").read()
        want = src.split("\n", 1)[0].removeprefix("// backstop: ")
        it = Interpreter(src, os.path.relpath(path, ROOT))
        sys.stderr = open(os.devnull, "w")
        try:
            it.run_main([])
            observed = "|".join(it.out)
        except DefectSignal as d:
            observed = "|".join(it.out + [f"defect:{d.kind}"])
        except FortisError as e:
            observed = "|".join(it.out + [f"dynamic:{e}"])
        finally:
            sys.stderr = sys.__stderr__
        ok = observed == want
        failures += not ok
        print(f"{'PASS' if ok else 'FAIL'} backstop {name:<27} {'' if ok else observed}")
    return failures


def negative_test() -> int:
    """A `// reject:` fixture tagged `requires: runtime` must be refused, not run."""
    import tempfile
    d = tempfile.mkdtemp()
    open(os.path.join(d, "bad.ft"), "w").write("// reject: E-OWN-020\n// requires: runtime\nexport func main(args: List[String]) Void {}\n")
    global PROGRAMS
    saved, PROGRAMS = PROGRAMS, d
    try:
        import io, contextlib
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = main([])
        refused = code == 1 and "may not be tagged" in buf.getvalue()
    finally:
        PROGRAMS = saved
    print("negative test (reject fixture tagged runtime is refused):", "PASS" if refused else "FAIL")
    return 0 if refused else 1


def main(argv):
    if "--negative" in argv:
        return negative_test()
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    results, failures = [], 0
    for name in sorted(os.listdir(PROGRAMS)):
        if not name.endswith(".ft"):
            continue
        path = os.path.join(PROGRAMS, name)
        src = open(path, encoding="utf-8").read()
        req = REQ.search(src)
        tags = [t.strip() for t in req.group(1).split(",")] if req else []
        if "runtime" not in tags:
            continue
        first = src.split("\n", 1)[0]
        if first.startswith("// reject"):
            results.append({"id": "PRG-" + name[:-3], "path": os.path.relpath(path, ROOT), "verdict": "reject", "status": "FAIL", "backend": "interpreter",
                            "commit": commit, "sha256": hashlib.sha256(src.encode()).hexdigest(), "command": "python3 -I conformance/tools/execute_check.py",
                            "expected": "refused", "observed": "a reject fixture tagged requires: runtime is not run", "codes": [], "stage": "core", "packages": []})
            print(f"FAIL {name}: a `// reject:` fixture may not be tagged `requires: runtime`")
            failures += 1
            continue
        m = MARK.search(src)
        if not m:
            continue
        expected = m.group(1).strip()
        sys.stderr = open(os.devnull, "w")
        try:
            observed, it = run_fixture(path)
        finally:
            sys.stderr = sys.__stderr__
        ok = observed == expected
        failures += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name:<36} {observed if not ok else ''}")
        results.append({"id": "PRG-" + name[:-3], "path": os.path.relpath(path, ROOT), "verdict": "execute", "status": "PASS" if ok else "FAIL",
                        "backend": "interpreter", "commit": commit, "sha256": hashlib.sha256(src.encode("utf-8")).hexdigest(),
                        "command": "python3 -I conformance/tools/execute_check.py", "expected": expected, "observed": observed,
                        "codes": [], "stage": "core", "packages": [], "reports": it.reports if it is not None else [],
                        "trace": list(it.trace) if it is not None else []})
    if PROGRAMS.startswith(os.path.join(ROOT, "conformance")):
        json.dump({"results": results}, open(os.path.join(ROOT, "conformance", "execution", "results.json"), "w", encoding="utf-8"), indent=2)
        failures += backstop_test()
    print(f"\n{len(results)} execute fixtures, {failures} FAIL")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
