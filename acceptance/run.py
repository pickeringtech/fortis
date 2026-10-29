"""The acceptance harness (Programme 14): runs one acceptance application against its specification and writes
a run log whose three evidence fields are kept separate (Programme 6.3, PROG-VM-03).

  python3 -I acceptance/run.py a [--backend interpreter]

The application directory holds `project.yaml`, `src/`, `data/` (the development FileSystem's contents for the
run), `expected/` (the hand-verified output), `negatives/` (one module variant per diagnostic class) and
`spec.json`. The harness:

  1. checks the package with `bin/fortis check` (0 findings);
  2. runs it with `bin/fortis run DIR --inventory` on a fresh copy of `data/`, compares stdout with the expected
     output and the runtime records (failures by code, observation frames) with the specification, and checks
     the runtime inventory (no task, pool, DI registry or exporter unless the specification requests one);
  3. runs every test with `bin/fortis test DIR` and checks the totals and that every test a feature names passed;
  4. checks each feature's source patterns and the run lines it names;
  5. substitutes each negative for its base module in a copy of the package: it must be rejected with its
     class's code within 6 lines of the change, with no record of another class, by a change of at most 8 lines,
     and its base is accepted (1);
     every diagnostic class whose producer exists (all of Programme 7.1 but E-LIM and E-INT) has one;
  6. applies each mutant (a control on the tests' power): the mutated package must check clean (a rejected
     mutant does not count) and the named test must fail, or the run's output must differ;
  7. writes `<app>/run-log.json`: evidence_integrity (sha256 of every input and output), execution_attestation
     (each command, where and when it ran, its exit status) and semantic_correctness (each check's result)."""
from __future__ import annotations

import datetime
import difflib
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
FORTIS = os.path.join(ROOT, "bin", "fortis")
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
TOOLCHAIN = ["bin/fortis", "reference/grammar/fortis.ebnf", "reference/library", "checker", "interpreter", "library/fortis",
             "conformance/tools/names.py", "conformance/tools/parser.py", "conformance/tools/lexer.py",
             "conformance/tools/resolve.py", "conformance/tools/project.py", "conformance/tools/tree.py",
             "conformance/tools/diagnostics.py", "acceptance/run.py"]
# Programme 7.1: class -> code prefix; E-LIM and E-INT have no producer before DEL-45
CLASSES = {"Type": "E-TYP", "Ownership": "E-OWN", "Obligation": "E-OBL", "Effect": "E-EFF", "Refinement unproved": "E-REF",
           "Unsupported shape": "E-OWN-090", "Syntax": "E-SYN", "Names and modules": "E-NAM", "Patterns": "E-MAT",
           "Closures": "E-FUN", "Defer and async": ("E-DEF", "E-ASY"), "Observation": "E-OBS"}
WITHOUT_PRODUCER = {"Resource limit": "E-LIM", "Internal defect": "E-INT"}
MAX_CHANGE = 8
NEAR = 6               # a record may point at the use, binding or next token rather than the edited line itself


def sha(path: str) -> str:
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def files_under(rel: str):
    p = os.path.join(ROOT, rel)
    if os.path.isfile(p):
        yield rel
        return
    for base, dirs, files in os.walk(p):
        dirs[:] = sorted(d for d in dirs if d != "__pycache__")
        for f in sorted(files):
            if not f.endswith(".pyc"):
                yield os.path.relpath(os.path.join(base, f), ROOT)


def class_of(code: str) -> str:
    if code.startswith("E-OWN-090"):
        return "Unsupported shape"
    for k, v in CLASSES.items():
        if k != "Unsupported shape" and any(code.startswith(p + "-") for p in ((v,) if isinstance(v, str) else v)):
            return k
    return "?"


def relative(value):
    """`value` with this checkout's absolute path written relative to the repository root, and the temporary
    directory written `<tmp>`, in every string: the log is committed, so it names no local directory (a temporary
    directory under build/ stays `build/tmp/...`)."""
    if isinstance(value, str):
        value = value.replace(ROOT + os.sep, "").replace(ROOT, ".")
        tmp = tempfile.gettempdir()
        return value.replace(tmp + os.sep, "<tmp>/") if tmp not in ("/", "") else value
    if isinstance(value, list):
        return [relative(v) for v in value]
    if isinstance(value, dict):
        return {k: relative(v) for k, v in value.items()}
    return value


class Harness:
    def __init__(self, app: str):
        self.app = app
        self.dir = os.path.join(ROOT, "acceptance", app)
        self.spec = json.load(open(os.path.join(self.dir, "spec.json"), encoding="utf-8"))
        self.steps, self.outputs, self.checks = [], {}, []

    # ---- execution attestation ------------------------------------------------
    def run(self, label: str, argv, env_extra=None, cwd=ROOT, python=True):
        env = dict(os.environ)
        env.update(env_extra or {})
        start = datetime.datetime.now(datetime.timezone.utc)
        prefix = [sys.executable, "-I"] if python else []
        p = subprocess.run(prefix + argv, capture_output=True, text=True, cwd=cwd, env=env, timeout=600)
        end = datetime.datetime.now(datetime.timezone.utc)
        self.steps.append({"step": label, "argv": (["python3", "-I"] if python else []) + [os.path.relpath(a, ROOT) if os.path.isabs(a) and a.startswith(ROOT) else a for a in argv],
                           "cwd": os.path.relpath(cwd, ROOT) or ".", "env": {k: v for k, v in (env_extra or {}).items()},
                           "started": start.isoformat(timespec="milliseconds"), "duration_ms": int((end - start).total_seconds() * 1000), "exit": p.returncode})
        return p

    def output(self, label: str, text: str):
        self.outputs[label] = hashlib.sha256(text.encode("utf-8")).hexdigest()

    def check(self, name: str, ok: bool, detail):
        self.checks.append({"check": name, "passed": bool(ok), "detail": detail})
        print(f"{'PASS' if ok else 'FAIL'} {name}" + (f": {detail}" if not ok or isinstance(detail, str) else ""))
        return ok

    # ---- the steps -------------------------------------------------------------
    def copy_package(self, into: str) -> str:
        name = re.search(r"^name: (\S+)", open(os.path.join(self.dir, "project.yaml")).read(), re.M).group(1)
        dst = os.path.join(into, name)
        shutil.copytree(self.dir, dst, ignore=shutil.ignore_patterns("negatives", "expected", "run-log.json", "spec.json"))
        return dst

    def records(self, stderr: str):
        out = []
        for line in stderr.splitlines():
            line = line.strip()
            if line.startswith("{"):
                try:
                    out.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
        return out

    def check_package(self):
        p = self.run("check", [FORTIS, "check", self.dir, "--format", "json"])
        self.output("check.stdout", p.stdout)
        findings = [json.loads(l) for l in p.stdout.splitlines() if l.strip().startswith("{")]
        self.check("package checks with 0 findings", p.returncode == 0 and not findings,
                   {"findings": len(findings), "codes": sorted({f["code"] for f in findings})})

    def run_app(self):
        root = tempfile.mkdtemp(prefix=f"acceptance-{self.app}-")
        shutil.copytree(os.path.join(self.dir, self.spec["data"]), root, dirs_exist_ok=True)
        p = self.run("run", [FORTIS, "run", self.dir, "--inventory", "--format", "json"], {"FORTIS_DEV_ROOT": root, **self.spec.get("env", {})})
        self.steps[-1]["env"] = {"FORTIS_DEV_ROOT": f"<fresh copy of acceptance/{self.app}/{self.spec['data']}>"}
        self.output("run.stdout", p.stdout)
        self.output("run.stderr", p.stderr)
        self.stdout = p.stdout
        expected = open(os.path.join(self.dir, self.spec["expected_stdout"]), encoding="utf-8").read()
        diff = list(difflib.unified_diff(expected.splitlines(), p.stdout.splitlines(), "expected", "observed", lineterm="", n=0))
        self.check("run exits 0", p.returncode == 0, {"exit": p.returncode})
        self.check("run output equals the expected output", not diff, {"lines": len(p.stdout.splitlines()), "diff": diff[:20]})
        recs = self.records(p.stderr)
        failures = {}
        for r in recs:
            if r.get("kind") == "runtime-failure":
                failures[r["code"]] = failures.get(r["code"], 0) + 1
        exp = self.spec["expected_records"]
        self.check("runtime failures reported once each, by code", failures == exp["runtime-failure"], {"observed": failures, "expected": exp["runtime-failure"]})
        frames = [{k: r.get(k) for k in ("schema", "function", "outcome", "fields")} for r in recs if r.get("kind") == "observation-frame"]
        self.check("observation frames", frames == exp["observation-frame"], {"observed": frames})
        inv = next((r for r in recs if r.get("kind") == "runtime-inventory"), None)
        want = self.spec["runtime_inventory"]
        ok = inv is not None and all(inv.get(k) == v for k, v in want.items())
        self.check("no unrequested runtime work (inventory)", ok, {"observed": inv, "expected": want})
        # a record names a package's module relative to the package's directory (D-302)
        origins = sorted({r["origin"]["file"] for r in recs if r.get("kind") == "runtime-failure"})
        if self.spec.get("min_failure_origins", 2):
            self.check("failure origins name the declaring module", all(o.startswith("src/") and os.path.isfile(os.path.join(self.dir, o)) for o in origins) and len(origins) >= self.spec.get("min_failure_origins", 2), {"origins": origins})

    # ---- every run on every backend (A2: Programme 14 asks both backends, DW-05) ----------------------------------
    def configs(self):
        """The main run and the spec's further runs: (label, data, env, expected stdout, exit, expected failure counts)."""
        main = ("main", self.spec["data"], self.spec.get("env", {}), self.spec["expected_stdout"], 0, self.spec["expected_records"]["runtime-failure"])
        extra = [(r["label"], r["data"], r.get("env", {}), r["expected_stdout"], r["exit"], r.get("expected_failures", {})) for r in self.spec.get("runs", [])]
        return [main] + extra

    def run_backend(self, backend: str, label, data, env, expected_path, want_exit, want_failures):
        root = tempfile.mkdtemp(prefix=f"acceptance-{self.app}-{backend}-")
        shutil.copytree(os.path.join(self.dir, data), root, dirs_exist_ok=True)
        e = {"FORTIS_DEV_ROOT": root, **env}
        if backend == "interpreter":
            p = self.run(f"{label} on the interpreter", [FORTIS, "run", self.dir, "--format", "json"], e)
            stdout, stderr = p.stdout, p.stderr
        elif backend == "vm":
            p = self.run(f"{label} on the VM", [FORTISC, "vm", self.dir], e, python=False)
            obs = next((l[len("observed: "):] for l in p.stdout.splitlines() if l.startswith("observed: ")), "")
            parts = obs.split("|") if obs else []
            fail = [x for x in parts if x.startswith("fail:")]
            stdout = "".join(x + "\n" for x in parts if not x.startswith("fail:"))
            # the VM's records (`record: kind=…; code=…`): each reported failure once, the entry's included
            recs = [dict(kv.split("=", 1) for kv in l[len("record: "):].split("; ") if "=" in kv) for l in p.stdout.splitlines() if l.startswith("record: ")]
            stderr = "\n".join(json.dumps({"kind": r.get("kind"), "code": r.get("code")}) for r in recs)
            # the executor trace names each launch's pool (the VM runs cooperative tasks on coroutine threads, which
            # are not pool threads)
            trace = next((l[len("trace: "):] for l in p.stdout.splitlines() if l.startswith("trace: ")), "")
            ev = trace.split("|") if trace else []
            self.vm_pools = {"blocking": sum(1 for x in ev if x.endswith("on blocking")), "parallel": sum(1 for x in ev if x.endswith("on parallel"))}
            p = subprocess.CompletedProcess(p.args, 1 if fail else 0, stdout, stderr)
        else:
            p = self.run(f"{label} natively", [self.native_exe], {**e, "FORTIS_TRACE": "1"}, python=False)
            stdout, stderr = p.stdout, p.stderr
            for r in self.records(stderr):
                if r.get("kind") == "executor-trace":
                    ev = r.get("events", [])
                    self.native_pools = {"blocking": sum(1 for x in ev if x.endswith("on blocking")), "parallel": sum(1 for x in ev if x.endswith("on parallel"))}
        expected = open(os.path.join(self.dir, expected_path), encoding="utf-8").read()
        failures = {}
        for r in self.records(stderr):
            if r.get("kind") == "runtime-failure":
                failures[r["code"]] = failures.get(r["code"], 0) + 1
        duration = self.steps[-1]["duration_ms"] / 1000
        # The start-time budget is a performance check, judged only when FORTIS_ACCEPTANCE_TIMING=1 (the v1.0 release
        # check, on a quiet machine). Otherwise the duration is recorded but not judged.
        timed = os.environ.get("FORTIS_ACCEPTANCE_TIMING") == "1"
        within = f", within {self.spec.get('start_within_s', 2)} s" if timed else ""
        self.check(f"{backend}: {label}: output, exit {want_exit} and failures {want_failures or 'none'} as expected{within}",
                   stdout == expected and p.returncode == want_exit and failures == want_failures and (not timed or duration <= self.spec.get("start_within_s", 2.0)),
                   {"exit": p.returncode, "failures": failures, "seconds": duration, "diff": list(difflib.unified_diff(expected.splitlines(), stdout.splitlines(), lineterm="", n=0))[:10]})

    def run_backends(self):
        backends = self.spec.get("backends", [])
        if not backends:
            return
        if "native" in backends:
            exe = os.path.join(tempfile.mkdtemp(prefix=f"acceptance-{self.app}-bin-"), self.app)
            b = self.run("native build", [FORTISC, "build", "--format", "json", "--out", exe, self.dir], python=False)
            self.check("the application builds natively", b.returncode == 0, {"exit": b.returncode, "tail": (b.stdout + b.stderr)[-300:]})
            self.native_exe = exe
        for backend in backends:
            if backend == "native" and not os.path.exists(getattr(self, "native_exe", "")):
                continue
            for cfg in self.configs():
                self.run_backend(backend, *cfg)
        want = self.spec.get("pool_threads")
        if want and "vm" in backends:
            self.check("VM: pools as requested (executor trace)", getattr(self, "vm_pools", None) == want, {"observed": getattr(self, "vm_pools", None), "requested": want})
        if want and "native" in backends:
            self.check("native: pools as requested (executor trace)", getattr(self, "native_pools", None) == want, {"observed": getattr(self, "native_pools", None), "requested": want})

    def run_tests(self):
        p = self.run("test", [FORTIS, "test", self.dir, "--format", "json"])
        self.output("test.stdout", p.stdout)
        self.output("test.stderr", p.stderr)
        summary = next((json.loads(l) for l in p.stdout.splitlines() if '"test-summary"' in l), {})
        self.tests = {}
        for r in self.records(p.stderr):
            if r.get("kind") == "test-result":
                self.tests.setdefault(r["test"], []).append(r["status"])
        exp = self.spec["expected_tests"]
        self.check("tests pass", summary.get("passed") == exp["passed"] and summary.get("failed") == exp["failed"] and p.returncode == 0,
                   {"passed": summary.get("passed"), "failed": summary.get("failed"), "tests": len(self.tests)})
        if "vm" in self.spec.get("backends", []):
            v = self.run("test on the VM", [FORTISC, "test", "--format", "json", self.dir], python=False)
            vs = next((json.loads(l) for l in v.stdout.splitlines() if '"test-summary"' in l), {})
            self.check("tests pass on the VM (fortisc test)", v.returncode == 0 and vs.get("failed") == 0 and vs.get("passed", 0) > 0, vs)

    def check_features(self):
        for f in self.spec["features"]:
            missing = []
            for rel, pat in f.get("source", []):
                text = open(os.path.join(self.dir, rel), encoding="utf-8").read()
                if not re.search(pat, text, re.M):
                    missing.append(f"{rel}: /{pat}/")
            for t in f.get("tests", []):
                if not self.tests.get(t) or any(s != "pass" for s in self.tests[t]):
                    missing.append(f"test `{t}` did not pass")
            for pat in f.get("run", []):
                if not any(re.search(pat, l) for l in self.stdout.splitlines()):
                    missing.append(f"run output has no line /{pat}/")
            for n in f.get("negatives", []):
                if not os.path.exists(os.path.join(self.dir, "negatives", n + ".ft")):
                    missing.append(f"negative {n}.ft missing")
            self.check(f"feature: {f['feature']}", not missing, {"missing": missing, **({"deferred": f["deferred"]} if "deferred" in f else {})})

    def check_imports(self):
        bad = []
        for m in sorted(os.listdir(os.path.join(self.dir, "src"))):
            for line in open(os.path.join(self.dir, "src", m), encoding="utf-8"):
                im = re.match(r"import ([\w.]+)", line)
                if im and not any(im.group(1) == a or im.group(1).startswith(a) for a in self.spec["allowed_imports"]):
                    bad.append(f"{m}: {im.group(1)}")
        self.check("imports no kit package", not bad, {"outside": bad, "allowed": self.spec["allowed_imports"]})

    def substitute(self, rel: str, text: str):
        d = tempfile.mkdtemp(prefix=f"acceptance-{self.app}-variant-")
        pkg = self.copy_package(d)
        open(os.path.join(pkg, rel), "w", encoding="utf-8").write(text)
        return pkg

    def check_negatives(self):
        if self.spec.get("negatives") is False:
            return
        seen = {}
        for f in sorted(os.listdir(os.path.join(self.dir, "negatives"))):
            src = open(os.path.join(self.dir, "negatives", f), encoding="utf-8").read()
            head, _, body = src.partition("\n// change:")
            base = re.search(r"^// base: (\S+)", src, re.M).group(1)
            cls_prefix = re.search(r"^// class: (\S+)", src, re.M).group(1)
            change = body.split("\n", 1)[0].strip()
            variant = body.split("\n", 1)[1]
            original = open(os.path.join(self.dir, base), encoding="utf-8").read()
            sm = difflib.SequenceMatcher(None, original.splitlines(), variant.splitlines())
            touched, changed = set(), 0
            for tag, i1, i2, j1, j2 in sm.get_opcodes():
                if tag != "equal":
                    changed += max(i2 - i1, j2 - j1)
                    touched |= set(range(j1 + 1, max(j2, j1 + 1) + 1))      # 1-based lines of the variant, and the line after a deletion
            pkg = self.substitute(base, variant)
            p = self.run(f"negative {f}", [FORTIS, "check", pkg, "--format", "json"])
            recs = [json.loads(l) for l in p.stdout.splitlines() if l.strip().startswith("{")]
            codes = sorted({r["code"] for r in recs})
            want_class = class_of(cls_prefix + ("-001" if not cls_prefix.endswith("090") else ""))
            at_change = [r for r in recs if class_of(r["code"]) == want_class and isinstance(r.get("line"), int)
                         and min(abs(r["line"] - t) for t in touched) <= NEAR]
            only_class = all(class_of(r["code"]) == want_class for r in recs)
            ok = bool(at_change) and only_class and changed <= MAX_CHANGE
            seen.setdefault(want_class, []).append(f)
            self.check(f"negative {f} ({want_class})", ok, {"change": change, "base": base, "changed_lines": changed,
                                                           "changed_at": sorted(touched)[:MAX_CHANGE + 1],
                                                           "codes": codes, "near_change": [f"{r['code']}@{r['line']}" for r in at_change]})
        missing = [c for c in CLASSES if c not in seen]
        self.check("one negative per diagnostic class with a producer", not missing,
                   {"classes": {c: seen.get(c, []) for c in CLASSES}, "missing": missing, "without_producer": WITHOUT_PRODUCER})

    def check_mutants(self):
        for i, m in enumerate(self.spec["mutants"], 1):
            original = open(os.path.join(self.dir, m["file"]), encoding="utf-8").read()
            if original.count(m["old"]) != 1:
                self.check(f"mutant {i} ({m['file']})", False, "its target text no longer occurs exactly once")
                continue
            pkg = self.substitute(m["file"], original.replace(m["old"], m["new"]))
            c = self.run(f"mutant {i} check", [FORTIS, "check", pkg, "--format", "json"])
            if c.returncode != 0:
                self.check(f"mutant {i} ({m['file']})", False, "the mutant is rejected by the checker, so it cannot count as killed")
                continue
            if m["killed_by"] == "test":
                p = self.run(f"mutant {i} test", [FORTIS, "test", pkg, "--format", "json"])
                statuses = [r["status"] for r in self.records(p.stderr) if r.get("kind") == "test-result" and r.get("test") == m["test"]]
                killed = p.returncode != 0 and any(s != "pass" for s in statuses)
                detail = {"mutation": f"{m['old'].strip()}  ->  {m['new'].strip()}", "killed_by": f"test `{m['test']}`", "statuses": statuses}
            else:
                root = tempfile.mkdtemp(prefix=f"acceptance-{self.app}-mutant-")
                shutil.copytree(os.path.join(self.dir, self.spec["data"]), root, dirs_exist_ok=True)
                p = self.run(f"mutant {i} run", [FORTIS, "run", pkg, "--format", "json"], {"FORTIS_DEV_ROOT": root})
                self.steps[-1]["env"] = {"FORTIS_DEV_ROOT": f"<fresh copy of acceptance/{self.app}/{self.spec['data']}>"}
                killed = p.stdout != self.stdout
                detail = {"mutation": f"{m['old'].strip()}  ->  {m['new'].strip()}", "killed_by": "run output differs"}
            self.check(f"mutant {i} killed ({m['file']})", killed, detail)

    # ---- the log -----------------------------------------------------------------
    def main(self) -> int:
        inputs = {}
        for rel in [os.path.relpath(os.path.join(self.dir, x), ROOT) for x in ("project.yaml", "spec.json")] + \
                   list(files_under(f"acceptance/{self.app}/src")) + list(files_under(f"acceptance/{self.app}/data")) + \
                   list(files_under(f"acceptance/{self.app}/expected")) + list(files_under(f"acceptance/{self.app}/negatives")):
            inputs[rel] = sha(os.path.join(ROOT, rel))
        toolchain = {rel: sha(os.path.join(ROOT, rel)) for t in TOOLCHAIN for rel in files_under(t)}
        self.check_package()
        self.run_app()
        self.run_backends()
        self.run_tests()
        self.check_features()
        self.check_imports()
        self.check_negatives()
        self.check_mutants()
        commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
        dirty = subprocess.run(["git", "status", "--porcelain", "--", "acceptance", "checker", "interpreter", "library", "reference", "bin", "conformance/tools"],
                               capture_output=True, text=True, cwd=ROOT).stdout.strip()
        dirty = "\n".join(l for l in dirty.splitlines() if not l.endswith("/run-log.json"))      # the log is this run's output
        digest = hashlib.sha256(json.dumps({"inputs": inputs, "toolchain": toolchain, "outputs": self.outputs}, sort_keys=True).encode()).hexdigest()
        failed = [c["check"] for c in self.checks if not c["passed"]]
        log = {
            "application": self.spec["application"], "title": self.spec["title"], "programme": self.spec["programme"],
            "chunk": self.spec["chunk"], "backend": self.spec["backend"],
            "evidence_integrity": {"algorithm": "sha256", "inputs": inputs, "toolchain": toolchain, "outputs": self.outputs, "digest": digest},
            # no host name: the log is committed, and the attestation needs the platform, not the machine's name
            "execution_attestation": {"commit": commit, "worktree_clean": not dirty, "platform": platform.platform(),
                                      "python": platform.python_version(), "harness": "python3 -I acceptance/run.py " + self.app, "steps": self.steps},
            "semantic_correctness": {"passed": not failed, "checks": self.checks, "failed": failed},
        }
        out = os.path.join(self.dir, "run-log.json")
        json.dump(relative(log), open(out, "w", encoding="utf-8"), indent=1)
        print(f"\n{len(self.checks)} checks, {len(failed)} failed; {len(self.steps)} commands; run log {os.path.relpath(out, ROOT)} (digest {digest[:16]})")
        return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(Harness(sys.argv[1] if len(sys.argv) > 1 else "a").main())
