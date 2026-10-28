"""DEL-43 criterion 1: one record identity across the batch commands and the query database (PROG-DIA-05,
PROG-DIA-07), with violated, unproved and unknown distinct (PROG-DIA-02).

`python3 -I conformance/tools/batch_check.py [--json]`

1. **Equality.** For every module of the corpus (conformance/programs, conformance/corpus, examples), the records
   `fortisc check` writes to a pipe are byte-equal, in order, to the query database's for the same snapshot
   (`fortis-query-db records`, the surface the daemon and agent protocol serve). `fortisc test` writes exactly the
   same records before it runs anything.
2. **Pipe and terminal.** To a pipe, `check` writes JSON lines, identical to `--format json`. On a terminal (a
   pseudo-terminal) it writes text, identical to `--format text` and to the reference renderer's text for the JSON
   records (conformance/tools/diagnostics.py), so the human text is rendered from the record and carries all of it.
   `--format json` on a terminal and `--format text` on a pipe override the default.
3. **Schema.** Every diagnostic record is valid (diagnostics.validate) and every record of `fortisc test` is valid
   against the runtime schema (interpreter/records.py), which also renders the text `test` prints on a terminal.
4. **Statuses.** Each marked line of the refinement, effect and foreign-boundary fixtures names the status it
   expects; the records carry exactly those statuses, and violated, unproved and unknown all occur.
"""
import json
import os
import pty
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
sys.path.insert(0, HERE)
sys.path.insert(0, ROOT)
import diagnostics  # noqa: E402
from interpreter import records as runtime  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
QUERYDB = os.path.join(ROOT, "compiler", "target", "release", "fortis-query-db")
STATUS_FIXTURES = ["conformance/programs/ref_statuses.ft", "conformance/programs/eff_statuses.ft", "conformance/programs/eff_foreign_statuses.ft"]


def corpus():
    files = sorted(os.path.join(d, f) for d in ("conformance/programs", "conformance/corpus", "examples") for f in os.listdir(os.path.join(ROOT, d)) if f.endswith(".ft"))
    # the Dependency injection corpus, read elaborated by both surfaces, and a module whose elaboration fails and whose
    # text does not parse, which both report by the kit's finding alone (D-139)
    files += sorted(os.path.join("conformance", "di", f) for f in os.listdir(os.path.join(ROOT, "conformance", "di")) if f.endswith(".ft"))
    return files + [os.path.join("conformance", "packages", "wiring.broken", "src", "app.ft")]


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, cwd=ROOT, timeout=3600, **kw)


def tty(args):
    """A command's output through a pseudo-terminal."""
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(ROOT)
        os.execv(args[0], args)
    data = b""
    while True:
        try:
            chunk = os.read(fd, 1 << 16)
        except OSError:
            break
        if not chunk:
            break
        data += chunk
    os.waitpid(pid, 0)
    return data.decode("utf-8", "replace").replace("\r\n", "\n")


def main(argv):
    rows, problems = [], 0

    def expect(name, ok, detail=""):
        nonlocal problems
        rows.append({"check": name, "ok": bool(ok), "detail": str(detail)[:600]})
        problems += not ok
        print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok else f": {str(detail)[:400]}"))

    files = corpus()
    # 1. equality with the query database, file by file
    piped = run([FORTISC, "check"] + files).stdout.splitlines()
    db = run([QUERYDB, "records"] + files).stdout.splitlines()
    by_file = lambda lines: [(json.loads(l)["file"], l) for l in lines]  # noqa: E731
    differ = sorted({f for f, _ in set(by_file(piped)) ^ set(by_file(db))})
    expect(f"check and the query database: byte-equal records for {len(files)} modules ({len(piped)} records)", piped == db, differ[:5])
    tested = run([FORTISC, "test"] + files, env={**os.environ, "NO_COLOR": "1"}).stdout.splitlines()
    is_diag = lambda l: "class" in json.loads(l) and "code" in json.loads(l)  # noqa: E731
    diag = [l for l in tested if is_diag(l)]
    # then the plan step's records (E-EFF-030), which check never writes (PROG-VM-04)
    plan = [l for l in diag if json.loads(l)["code"] == "E-EFF-030"]
    expect("test writes the records check writes, before anything runs (then the plan step's)", [l for l in diag if l not in plan] == piped and len(plan) == 1, f"{len(diag)} against {len(piped)}")
    # 2. pipe, terminal and the override
    sample = [f for f in STATUS_FIXTURES] + ["conformance/programs/syntax_semicolon.ft", "conformance/programs/syntax_logical_then_coalesce.ft"]
    as_json = run([FORTISC, "check", "--format", "json"] + sample).stdout
    expect("to a pipe, check writes JSON lines, identical to --format json", run([FORTISC, "check"] + sample).stdout == as_json and all(l.startswith("{") for l in as_json.splitlines()))
    text = tty([FORTISC, "check"] + sample)
    rendered = "".join(diagnostics.render_text(json.loads(l)) + "\n" for l in as_json.splitlines())
    expect("on a terminal, check writes text rendered from the records (the reference renderer's text)", text == rendered, next((f"{a!r} / {b!r}" for a, b in zip(text.splitlines(), rendered.splitlines()) if a != b), ""))
    expect("--format text on a pipe writes the terminal's text", run([FORTISC, "check", "--format", "text"] + sample).stdout == text)
    expect("--format json on a terminal writes JSON lines", tty([FORTISC, "check", "--format", "json"] + sample) == as_json)
    # the reference command renders the same record to the same text (D-55)
    fortis = os.path.join(ROOT, "bin", "fortis")
    ref_text = "".join(run([sys.executable, "-I", fortis, "check", "--format", "text", f]).stdout for f in sample)
    expect("`fortis check --format text` writes fortisc's text for the same records", ref_text == text, next((f"{a!r} / {b!r}" for a, b in zip(ref_text.splitlines(), text.splitlines()) if a != b), ""))
    # an input that does not exist is an invalid invocation, exit status 2 (Programme 9.2; D-54, D-328)
    missing = os.path.join(ROOT, "build", "no-such-module.ft")
    statuses = {f"fortisc {c}": run([FORTISC, c, missing]).returncode for c in ("check", "build", "test", "vm", "native", "fmt")}
    statuses.update({f"fortis {c}": run([sys.executable, "-I", fortis, c, missing]).returncode for c in ("check", "run", "test", "build", "fmt")})
    expect("a missing input file is exit status 2 for every command", set(statuses.values()) == {2}, statuses)
    missing_dir = os.path.join(ROOT, "build", "no-such-rcf-dir")
    dir_statuses = {"fortisc fmt": run([FORTISC, "fmt", missing_dir]).returncode, "fortis fmt": run([sys.executable, "-I", fortis, "fmt", missing_dir]).returncode}
    expect("a missing directory is exit status 2 for fmt", set(dir_statuses.values()) == {2}, dir_statuses)
    present = "conformance/programs/exec_ordinary_comment.ft"
    expect("fmt --check of a present file exits 0", run([FORTISC, "fmt", "--check", present]).returncode == 0 and run([sys.executable, "-I", fortis, "fmt", "--check", present]).returncode == 0)
    # a failing try main reports once on stderr and exits 1, on every runner (LR 4.6, D-325)
    fail = "conformance/programs/exec_main_failure.ft"
    runners = {
        "interpreter": run([sys.executable, "-I", fortis, "run", fail, "--format", "json"]),
        "vm": run([FORTISC, "vm", fail]),
        "native": None,
    }
    native_bin = os.path.join(ROOT, "build", "runfile", "exec_main_failure")
    os.makedirs(os.path.dirname(native_bin), exist_ok=True)
    if os.path.lexists(native_bin):
        os.remove(native_bin)
    built = run([FORTISC, "build", "--out", native_bin, fail])
    runners["native"] = run([native_bin]) if built.returncode == 0 and os.path.exists(native_bin) else built

    def failure_records(err):
        out = []
        for line in err.splitlines():
            if not line.strip():
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("kind") == "runtime-failure":
                out.append(rec)
        return out

    fail_recs = {who: failure_records(p.stderr) for who, p in runners.items()}
    fail_ok = (
        built.returncode == 0
        and all(p.returncode == 1 for p in runners.values())
        and all(len(rs) == 1 and rs[0].get("code") == "demo.boom" and rs[0].get("rule") == "LANG-10-06" for rs in fail_recs.values())
        and len({json.dumps(rs[0], sort_keys=True) for rs in fail_recs.values() if rs}) == 1
        and runners["interpreter"].stdout == ""
        and runners["native"].stdout == ""
        and "observed: fail:demo.boom" in runners["vm"].stdout.splitlines()
    )
    expect("a failing main exits 1 and reports one runtime-failure, on every runner", fail_ok,
           {who: (p.returncode, p.stdout[-120:], p.stderr[-300:]) for who, p in runners.items()})
    # a `// tests: run` comment does not stop main; `--tests` still runs tests; an ordinary comment runs main (D-326)
    commented = "conformance/programs/exec_comment_runs_main.ft"
    ordinary = present

    def program_out(path):
        ref = run([sys.executable, "-I", fortis, "run", path])
        vm = run([FORTISC, "vm", path])
        obs = next((l[len("observed: "):] for l in vm.stdout.splitlines() if l.startswith("observed: ")), "")
        dest = os.path.join(ROOT, "build", "runfile", os.path.basename(path) + ".bin")
        if os.path.lexists(dest):
            os.remove(dest)
        bld = run([FORTISC, "build", "--out", dest, path])
        native = run([dest]) if bld.returncode == 0 and os.path.exists(dest) else bld
        return ref, vm, obs, native

    ref, vm, obs, native = program_out(commented)
    expect("a // tests: run comment still runs main", ref.returncode == 0 and ref.stdout == "hello\n" and "passed" not in ref.stdout
           and vm.returncode == 0 and obs == "hello" and native.returncode == 0 and native.stdout == "hello\n",
           (ref.returncode, ref.stdout, vm.returncode, obs, native.returncode, native.stdout[-80:]))
    tests = run([FORTISC, "vm", "--tests", commented])
    tests_obs = next((l[len("observed: "):] for l in tests.stdout.splitlines() if l.startswith("observed: ")), "")
    expect("--tests runs tests instead of main", tests.returncode == 0 and tests_obs == "0 passed, 0 failed" and "hello" not in tests.stdout, tests_obs)
    ref, vm, obs, native = program_out(ordinary)
    expect("an ordinary comment runs main", ref.returncode == 0 and ref.stdout == "noted\n" and vm.returncode == 0 and obs == "noted"
           and native.returncode == 0 and native.stdout == "noted\n",
           (ref.returncode, ref.stdout, vm.returncode, obs, native.returncode, getattr(native, "stdout", "")[-80:]))

    # a report inside a task reaches stderr on every runner, and the reporter stays bounded (D-325 review)
    def write_prog(name, src):
        folder = os.path.join(ROOT, "build", "rcf-bounds")
        os.makedirs(folder, exist_ok=True)
        dest = os.path.join(folder, name)
        with open(dest, "w", encoding="utf-8") as fh:
            fh.write(src)
        return os.path.relpath(dest, ROOT)

    def three(path):
        ref_p = run([sys.executable, "-I", fortis, "run", path, "--format", "json"])
        vm_p = run([FORTISC, "vm", path])
        dest = os.path.join(ROOT, "build", "runfile", os.path.basename(path) + ".bin")
        if os.path.lexists(dest):
            os.remove(dest)
        bld = run([FORTISC, "build", "--out", dest, path])
        native_p = run([dest]) if bld.returncode == 0 and os.path.exists(dest) else bld
        return ref_p, vm_p, native_p, bld

    task = write_prog("report_task.ft", """export error Boom {
  code: "demo.boom"
}

try func boom() Int {
  fail Boom()
}

async func worker() Void {
  let v = boom() catch e {
    report e
    recover 0
  }
  discard v
}

export async func main(args: List[String]) Void {
  scope tasks {
    let t = tasks.launch(worker())
    await t
  }
}
""")
    ref, vm, native, bld = three(task)
    task_recs = {"interpreter": failure_records(ref.stderr), "vm": failure_records(vm.stderr), "native": failure_records(native.stderr)}
    task_ok = (
        bld.returncode == 0
        and ref.returncode == 0 and vm.returncode == 0 and native.returncode == 0
        and all(len(rs) == 1 and rs[0].get("code") == "demo.boom" and rs[0].get("reported_by") == "report" for rs in task_recs.values())
        and len({json.dumps(rs, sort_keys=True) for rs in task_recs.values()}) == 1
    )
    expect("a task's report reaches stderr on every runner", task_ok,
           {who: (p.returncode, p.stderr[-200:]) for who, p in (("interpreter", ref), ("vm", vm), ("native", native))})

    over = write_prog("overflow.ft", """export error Boom {
  code: "demo.boom"
}

try func boom() Int {
  fail Boom()
}

export try func main(args: List[String]) Void {
  let mut i = 0
  while i < 1001 {
    let v = boom() catch e {
      report e
      recover 0
    }
    discard v
    i = Int.add(i, 1) or fail
  }
}
""")
    ref, vm, native, bld = three(over)

    def split_err(err):
        fails, overs = [], []
        for line in err.splitlines():
            if not line.strip():
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("kind") == "runtime-failure":
                fails.append(rec)
            elif rec.get("kind") == "buffer-overflow":
                overs.append(rec)
        return fails, overs

    parts = {"interpreter": split_err(ref.stderr), "vm": split_err(vm.stderr), "native": split_err(native.stderr)}
    over_ok = (
        bld.returncode == 0
        and ref.returncode == 0 and vm.returncode == 0 and native.returncode == 0
        and ref.stderr == vm.stderr == native.stderr
        and all(len(fails) == 999 and len(overs) == 1 and overs[0].get("bound") == 1000 and overs[0].get("dropped") == 2
                and overs[0].get("rule") == "PROG-NFR-13" for fails, overs in parts.values())
    )
    expect("a 1001st failure report is one buffer-overflow on every runner", over_ok,
           {who: (len(fails), [o.get("dropped") for o in overs], p.stderr[-120:])
            for (who, (fails, overs)), p in zip(parts.items(), (ref, vm, native))})
    # 3. schema
    invalid = [(l[:80], e) for l in piped for e in diagnostics.validate(json.loads(l))]
    expect(f"every diagnostic record is schema-valid ({len(piped)})", not invalid, invalid[:3])
    bad = []
    runtime_recs = [json.loads(l) for l in tested if not is_diag(l)]
    for r in runtime_recs:
        try:
            runtime.validate(r)
        except runtime.RecordError as e:
            bad.append(str(e))
    kinds = sorted({r["kind"] for r in runtime_recs})
    expect(f"every record of test is valid against the runtime schema ({len(runtime_recs)} records: {', '.join(kinds)})", not bad and runtime_recs, bad[:3])
    # 4. statuses
    seen = set()
    for f in STATUS_FIXTURES:
        src = open(os.path.join(ROOT, f), encoding="utf-8").read().split("\n")
        want = {i: m.group(1) for i, l in enumerate(src, start=1) for m in [re.search(r"//\s*reject:\s*E-[A-Z]+-\d+\s*—\s*(violated|unproved|unknown)\b", l)] if m and not l.lstrip().startswith("//")}
        got = {}
        for l in run([FORTISC, "check", "--format", "json", f]).stdout.splitlines():
            r = json.loads(l)
            got.setdefault(r["line"], set()).add(r["status"])
        ok = set(got) == set(want) and all(got[i] == {want[i]} for i in want)
        seen |= set(want.values())
        expect(f"{f}: statuses {sorted(set(want.values()))} at the marked lines", ok, f"want {want}, got {got}")
    expect("violated, unproved and unknown all occur and are distinct", seen == {"violated", "unproved", "unknown"}, sorted(seen))
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"rules": ["PROG-DIA-02", "PROG-DIA-05", "PROG-DIA-07"], "modules": len(files), "checks": rows}, open(os.path.join(ROOT, "build", "batch.json"), "w"), indent=1)
    print(f"\n{len(rows)} checks, {sum(r['ok'] for r in rows)} pass")
    print("batch check: " + ("OK" if problems == 0 else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
