#!/usr/bin/env python3
"""The SP-12 harness (spikes/SP-12/design.md 6-11): the daemon's runs, the batch check of every state they reach, and
the checks.

    python3 -I benchmarks/incremental/bench.py run --size 50k --out build/incremental/run-50k
    python3 -I benchmarks/incremental/bench.py verify --size 50k --out build/incremental/run-50k --workers 12
    python3 -I benchmarks/incremental/bench.py report --size 50k --out build/incremental/run-50k --json results.json
    python3 -I benchmarks/incremental/bench.py mutations --out build/incremental/mutations --json mutations.json

`run` makes, in order and one at a time:
- the cold checks: `fortisc check DIR` 5 times, and the daemon from process start to its `open` reply 3 times;
- the fixtures (fixtures/steps.json), every reply compared at once with the batch check of the same state;
- the measured latency run: one daemon process, `open`, the 1,070-edit script (the first 50 are warm-up), `stats`
  after every 100th edit;
- the measured soak: a second process, `open`, 10 cycles of 500 edits and their inverses, `stats` at every cycle end;
- the replay: a third process, the latency script again with `verify` at the deep points, then a fourth, the soak's
  first two cycles with `verify` at every 100th edit. No figure is taken from them: the measured processes never
  host a second database (design 9).
`verify` runs the batch check on every distinct state the runs reached, in a pool of processes, after the runs.
`report` computes every check from the logs and the batch answers, and replays the scripts to confirm that each log
holds the preregistered requests. Each log line is one request and its reply; each log ends with its terminal reason.
"""
import argparse
import hashlib
import json
import multiprocessing
import os
import platform
import queue
import shutil
import statistics
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import edits  # noqa: E402
import oracle  # noqa: E402

FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
FAULT_FORTISC = os.path.join(ROOT, "compiler", "target", "faults", "release", "fortisc")
SIZES = {"50k": 50000, "500k": 500000}
GIB = 1 << 30
COLD = {"50k": 5.0, "500k": 30.0}
HWM = {"50k": 2 * GIB, "500k": 8 * GIB}
SEED = 12
WARMUP = 50
FORWARD = 500
CYCLES = {"full": 10, "reduced": 4}
REPLAY_SOAK_CYCLES = {"full": 2, "reduced": 1}
REQUEST_TIMEOUT = 600
# a verify request has P's deadline, like every other request (closing review 6)
VERIFY_TIMEOUT = REQUEST_TIMEOUT
# G2 (SP-12b, X-141): the live bytes requested, the live allocations and the in-use bytes after `open` and at every
# soak cycle end must be equal: no tolerance (SP-12b design review 8)
LIVE_TOLERANCE = (0, 0, 0)
# G4, a budget: the allocator footprint's growth from soak cycle end 1 to the last, at most 8 MiB plus 0.5% of the
# footprint at cycle end 1 (the calibration's 10-cycle growth was 1.3 MB at 50k and 2.2 MB at 500k)
G4_ALLOWANCE = 8 << 20
G4_SHARE = 0.005
# and the resident set's peak, later cycle ends against earlier: 32 MiB plus 2% (allocator retention; a budget)
G3_ALLOWANCE = 32 << 20
ANSWERS_SCHEMA = "sp12-answers/2"
BODY = {"body-literal", "body-line", "body-error", "effect", "ownership", "obligation"}
SIGNATURE = {"sig-compat", "sig-break", "iface", "contract", "alias", "member", "decl"}
# kinds whose opening leaves the package to the resolver alone: no deep comparison in that state (design 9)
RESOLVER_ONLY = {"syntax", "rename"}


def deep_points(run, mode, items):
    """The steps of a replay whose state the daemon compares with a fresh database (design 7, V): every state the
    resolver does not answer alone; of those, in the latency replay, the first edit of each kind and role and every
    25th (10th reduced), and in the soak replay every 100th. The report regenerates them (re-review 1)."""
    out, firsts = [], set()
    for n, (_, e) in enumerate(items):
        if e.kind in RESOLVER_ONLY and e.role == "open":
            continue
        if run == "replay-latency":
            k = (e.kind, e.role)
            if k not in firsts or (n + 1) % (25 if mode == "full" else 10) == 0:
                firsts.add(k)
                out.append(n)
        elif run == "replay-soak" and (n + 1) % 100 == 0:
            out.append(n)
    return out
# the faults of the apparatus check and the families that must kill them (design 10)
FAULTS = {
    "foreign-dropped": {"R", "V"}, "foreign-module": {"I"}, "positional-heads": {"I"}, "interface-local": {"R", "V"},
    "summary-unchecked": {"R", "V"}, "dirty-shallow": {"R", "V"}, "resolver-stale": {"R"}, "reply-stale": {"R"},
    "wire-corrupt": {"R"}, "skip-final": {"R", "V"}, "omit-own": {"R"}, "omit-eff": {"R"}, "omit-obl": {"R"},
    "omit-ref": {"R"}, "hole-idents": {"R", "V"}, "alias-shape": {"R"}, "root-stale": {"R"}, "cycle-unordered": {"R"},
    "load-twice": {"R"}, "verify-broken": {"P", "V"}, "gc-off": {"G1"}, "leak-text": {"G2"}, "leak-small": {"G2"},
    "slow-check": {"L1", "L2"},
    # SP-12b (X-141): each mechanism of the replacement, undone
    "strings-fresh": {"G2"}, "fit-loose": {"G2"}, "log-kept": {"G2"}, "insert-unchecked": {"G2"}, "strs-kept": {"G2"},
    "dirty-kept": {"G2"}, "tables-kept": {"G2"}, "scratch-kept": {"G2"}, "system-mix": {"G2"}, "leak-uncounted": {"G4"},
    "free-leak": {"G4"},
}
FIXTURES = os.path.join("benchmarks", "incremental", "fixtures")
RUNS = ("latency", "soak", "replay-latency", "replay-soak")


def sha(text):
    return hashlib.sha256(text.encode()).hexdigest()


def sha_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def tree_digest(d):
    h = hashlib.sha256()
    for base, dirs, files in os.walk(d):
        dirs.sort()
        for f in sorted(files):
            p = os.path.join(base, f)
            h.update(os.path.relpath(p, d).encode() + b"\0" + sha_file(p).encode() + b"\n")
    return h.hexdigest()


def pkg_of(size):
    return os.path.join("build", "incremental", f"app-{size}")


def ensure_workload(size):
    d = os.path.join(ROOT, pkg_of(size))
    r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "generate.py"), "--lines", str(SIZES[size]), "--out", d], capture_output=True, text=True, check=True, timeout=600)
    return r.stdout.strip()


def identity(size):
    """What a batch answer is a function of besides the module texts (apparatus review 11): the batch fortisc, the
    manifest, a lockfile if any, the standard library's sources, and this cache's schema."""
    pkg = os.path.join(ROOT, pkg_of(size))
    lock = os.path.join(pkg, "fortis.lock")
    parts = [ANSWERS_SCHEMA, sha_file(FORTISC), sha_file(os.path.join(pkg, "project.yaml")), sha_file(lock) if os.path.exists(lock) else "-"]
    parts += [tree_digest(os.path.join(ROOT, d)) for d in ("library/fortis", "reference/library")]
    return sha("\n".join(parts))


def normalise(lines, pkg):
    prefix = pkg.rstrip("/") + "/"
    return [l.replace(prefix, "<pkg>/") for l in lines]


def batch(fortisc, d, timeout=1200):
    """`fortisc check --format json d`: (ok, record lines, status). The answer is valid only when the command exits 0
    with no error record or 1 with one, writes nothing to standard error, and every line is a record (apparatus
    review 5)."""
    try:
        r = subprocess.run([fortisc, "check", "--format", "json", d], cwd=ROOT, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return False, [], {"status": "timeout"}
    lines = [l for l in r.stdout.splitlines() if l.strip()]
    ok = r.returncode in (0, 1) and not r.stderr.strip()
    errors = False
    for l in lines:
        try:
            j = json.loads(l)
            errors |= j.get("severity") == "error"
            ok &= isinstance(j.get("code"), str) and isinstance(j.get("file"), str)
        except json.JSONDecodeError:
            ok = False
    ok &= (r.returncode == 1) == errors
    return ok, lines, {"status": r.returncode, "stderr": r.stderr[-500:]}


class States:
    """The package's texts and the digest of the whole state."""

    def __init__(self, pkg):
        man = json.load(open(os.path.join(ROOT, pkg, "manifest.json"), encoding="utf-8"))
        self.texts = {m["file"]: open(os.path.join(ROOT, pkg, m["file"]), encoding="utf-8").read() for m in man["modules"]}
        self.shas = {f: sha(t) for f, t in self.texts.items()}

    def set(self, f, text):
        self.texts[f] = text
        self.shas[f] = sha(text)

    def digest(self):
        return sha("".join(f"{f}\0{self.shas[f]}\n" for f in sorted(self.shas)))


def plan_of(mode):
    return edits.REDUCED if mode == "reduced" else edits.LATENCY


def stream(size, mode, run):
    """The edits a run sends: (cycle, edit) pairs."""
    pkg = os.path.join(ROOT, pkg_of(size))
    if run in ("latency", "replay-latency"):
        return ((0, e) for e in edits.Script(pkg, SEED, plan=plan_of(mode)).edits())
    # a soak cycle is drawn from the full script in both configurations, so it always has 500 edits
    cycles = CYCLES[mode] if run == "soak" else REPLAY_SOAK_CYCLES[mode]
    return edits.soak(pkg, SEED, cycles=cycles, forward=FORWARD, plan=edits.LATENCY)


class Daemon:
    def __init__(self, fortisc, pkg, env, errlog):
        self.err = open(errlog, "w")
        self.p = subprocess.Popen([fortisc, "daemon", pkg], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.err, cwd=ROOT, env=env, text=True, bufsize=1)
        self.q = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            self.q.put(line)
        self.q.put(None)

    def ask(self, req, timeout=REQUEST_TIMEOUT):
        """The reply and the wall time in ms, or (None, 'crash'|'timeout'|'malformed'). One deadline covers sending and
        receiving (apparatus review 10): a daemon that stops reading cannot block the harness."""
        line = json.dumps(req) + "\n"
        t0 = time.perf_counter()
        sent, failed = threading.Event(), []

        def write():
            try:
                self.p.stdin.write(line)
                self.p.stdin.flush()
            except (BrokenPipeError, OSError, ValueError) as e:
                failed.append(e)
            sent.set()
        threading.Thread(target=write, daemon=True).start()
        if not sent.wait(timeout):
            self.kill()
            return None, "timeout"
        if failed:
            return None, "crash"
        try:
            out = self.q.get(timeout=max(0.001, timeout - (time.perf_counter() - t0)))
        except queue.Empty:
            self.kill()
            return None, "timeout"
        t1 = time.perf_counter()
        if out is None:
            return None, "crash"
        try:
            return json.loads(out), (t1 - t0) * 1000.0
        except json.JSONDecodeError:
            return None, "malformed"

    def kill(self):
        try:
            self.p.kill()
        except OSError:
            pass

    def close(self):
        """Ask the daemon to exit, and reap it, within a bound either way."""
        sent = threading.Event()

        def write():
            try:
                self.p.stdin.write(json.dumps({"op": "exit"}) + "\n")
                self.p.stdin.flush()
                self.p.stdin.close()
            except (BrokenPipeError, OSError, ValueError):
                pass
            sent.set()
        threading.Thread(target=write, daemon=True).start()
        sent.wait(10)
        try:
            self.p.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.kill()
            self.p.wait(timeout=30)
        self.err.close()


def conditions():
    def read(p):
        try:
            return open(p).read().strip()
        except OSError:
            return None
    temps = []
    for z in sorted(os.listdir("/sys/class/thermal")) if os.path.isdir("/sys/class/thermal") else []:
        t = read(f"/sys/class/thermal/{z}/temp")
        if t and t.isdigit():
            temps.append(int(t) / 1000)
    return {
        "time": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "loadavg": os.getloadavg(),
        "governor": read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        "temperatures_c": temps,
        "kernel": platform.release(),
        "python": platform.python_version(),
        "rustc": subprocess.run(["rustc", "--version"], capture_output=True, text=True, timeout=60).stdout.strip(),
    }


def session(fortisc, size, mode, run, log_path, env, stop_after=None):
    """One daemon process: `open`, then every edit of the run's stream, one request at a time. The log ends with the
    terminal reason: `completed`, or `system-failure` with what failed (a crash, a timeout, a malformed or failed
    reply), which is a failed P check of the system and leaves the results complete."""
    pkg = pkg_of(size)
    st = States(pkg)
    out = open(log_path, "w")
    d = Daemon(fortisc, pkg, env, log_path + ".err")

    def write(entry):
        out.write(json.dumps(entry, sort_keys=True) + "\n")

    def end(reason, detail=None):
        write({"op": "end", "reason": reason, "detail": detail})
        out.close()
        d.close()

    def stats(after, **extra):
        """A `stats` request; a failed one ends the run as a failure of the system (re-review 2)."""
        s, ms = d.ask({"op": "stats"})
        write({"op": "stats", "after": after, **extra, "reply": s})
        return s is not None and s.get("ok") is True, ms if s is None else s.get("error")

    write({"op": "conditions", **conditions()})
    items = list(stream(size, mode, run))
    deep = set(deep_points(run, mode, items))
    t0 = time.perf_counter()
    r, ms = d.ask({"op": "open"})
    write({"op": "open", "state": st.digest(), "wall_ms": (time.perf_counter() - t0) * 1000.0, "reply": r})
    if r is None or not r.get("ok"):
        return end("system-failure", {"step": "open", "error": ms if r is None else r.get("error")})
    ok, err = stats("open")
    if not ok:
        return end("system-failure", {"step": "stats after open", "error": err})
    last_cycle, n = 0, 0
    for c, e in items:
        if stop_after is not None and n >= stop_after:
            break
        if c != last_cycle:
            ok, err = stats(f"cycle {last_cycle}", cycle=last_cycle)
            if not ok:
                return end("system-failure", {"step": f"stats at cycle {last_cycle}", "error": err})
            last_cycle = c
        st.set(e.file, e.text)
        r, ms = d.ask({"op": "edit", "file": e.file, "text": e.text})
        entry = {"op": "edit", "step": n, "cycle": c, "edit": e.describe(), "state": st.digest(), "reply": r}
        if r is None or not r.get("ok"):
            entry["error"] = ms if r is None else r.get("error")
            write(entry)
            return end("system-failure", {"step": n, "error": entry["error"]})
        entry["wall_ms"] = ms
        write(entry)
        n += 1
        if run == "latency" and n % 100 == 0:
            ok, err = stats(f"edit {n}")
            if not ok:
                return end("system-failure", {"step": f"stats after edit {n}", "error": err})
        if n - 1 in deep:
            v, vms = d.ask({"op": "verify"}, timeout=VERIFY_TIMEOUT)
            write({"op": "verify", "step": n - 1, "state": st.digest(), "reply": v, "error": None if v else vms})
            if v is None or v.get("ok") is not True:
                return end("system-failure", {"step": n - 1, "error": vms if v is None else v.get("error")})
    if run == "soak":
        ok, err = stats(f"cycle {last_cycle}", cycle=last_cycle)
        if not ok:
            return end("system-failure", {"step": f"stats at cycle {last_cycle}", "error": err})
    ok, err = stats("end")
    if not ok:
        return end("system-failure", {"step": "stats at the end", "error": err})
    return end("completed")


def fixtures_session(fortisc, out_dir, env):
    """The fixtures (design 3.4, 15): small packages, each edited by its steps, every reply compared at once with the
    batch check of the same state, written to a working copy and checked with the same fortisc (the lockfile
    fixture's records name the running compiler's digest)."""
    steps = json.load(open(os.path.join(ROOT, FIXTURES, "steps.json"), encoding="utf-8"))
    log = open(os.path.join(out_dir, "fixtures.jsonl"), "w")
    for name in sorted(k for k in steps if k != "about"):
        work = os.path.join("build", "incremental", "fixture-work", os.path.basename(out_dir.rstrip("/")), name)
        if os.path.isdir(os.path.join(ROOT, work)):
            shutil.rmtree(os.path.join(ROOT, work))
        shutil.copytree(os.path.join(ROOT, FIXTURES, name), os.path.join(ROOT, work))
        original = {}
        for base, _, files in os.walk(os.path.join(ROOT, work, "src")):
            for f in files:
                p = os.path.join(base, f)
                original[os.path.relpath(p, os.path.join(ROOT, work))] = open(p, encoding="utf-8").read()
        texts = dict(original)
        d = Daemon(fortisc, work, env, os.path.join(out_dir, f"fixture-{name}.err"))
        r, _ = d.ask({"op": "open"})
        ok, lines, status = batch(fortisc, work)
        log.write(json.dumps({"fixture": name, "op": "open", "reply": r, "batch": lines, "batch_ok": ok, "batch_status": status}) + "\n")
        for n, s in enumerate(steps[name]):
            f = s["file"]
            if s.get("restore"):
                texts[f] = original[f]
            elif "append" in s:
                texts[f] = texts[f] + s["append"]
            else:
                old, new = s["replace"]
                assert old in texts[f], (name, n, old)
                texts[f] = texts[f].replace(old, new, 1)
            r, _ = d.ask({"op": "edit", "file": f, "text": texts[f]})
            open(os.path.join(ROOT, work, f), "w", encoding="utf-8").write(texts[f])
            ok, lines, status = batch(fortisc, work)
            log.write(json.dumps({"fixture": name, "op": "edit", "step": n, "file": f, "reply": r, "batch": lines, "batch_ok": ok, "batch_status": status}) + "\n")
            if r is None:
                break
        d.close()
    log.write(json.dumps({"op": "end", "reason": "completed"}) + "\n")
    log.close()


WITNESS = "candidates"
COUNTS = ("live_bytes", "live_allocs", "in_use_bytes")


def witness_histories(texts):
    """The witness's requests: (op, point, file, text) in order (SP-12b apparatus reviews 4 and re-review 2, 3)."""
    b = texts["src/b.ft"] if texts["src/b.ft"].endswith("\n") else texts["src/b.ft"] + "\n"
    many = b + "".join(f"func w{i}() Int => {i}\n" for i in range(100))
    # the reviewer's chain: `entry` reaches the end of a 50-link chain, so the query recursion goes 50 deep
    chain = b + "func entry() Int => link50()\nfunc link0() Int => 1\n" + "".join(f"func link{i}() Int => link{i - 1}()\n" for i in range(1, 51))
    original = texts["src/b.ft"]
    return [("open", "open", None, None), ("stats", "open", None, None),
            ("edit", "tables", "src/b.ft", many), ("edit", "tables", "src/b.ft", original), ("stats", "tables", None, None),
            ("edit", "scratch", "src/b.ft", chain), ("edit", "scratch", "src/b.ft", original), ("edit", "scratch", "src/b.ft", original),
            ("stats", "scratch", None, None)]


def witness_session(fortisc, out_dir, env):
    """G2's witnesses: histories the soak never makes. On a working copy of a small fixture, the daemon is at its
    original texts after `open`, again after 100 functions are added to a module and removed (its tables grow past a
    threshold and must shrink back), and again after a 50-deep call chain is added, removed, and removed again as a
    no-op (its scratch stacks grow and must be released). The three points must have the same allocator counts.
    Every reply is compared at once with the batch check of its state, as the fixtures' are."""
    work = os.path.join("build", "incremental", "fixture-work", os.path.basename(out_dir.rstrip("/")), "witness")
    if os.path.isdir(os.path.join(ROOT, work)):
        shutil.rmtree(os.path.join(ROOT, work))
    shutil.copytree(os.path.join(ROOT, FIXTURES, WITNESS), os.path.join(ROOT, work))
    texts = {f"src/{f}": open(os.path.join(ROOT, work, "src", f), encoding="utf-8").read() for f in ("a.ft", "b.ft")}
    log = open(os.path.join(out_dir, "witness.jsonl"), "w")
    d = Daemon(fortisc, work, env, os.path.join(out_dir, "witness.err"))
    reason = "completed"
    for op, point, f, text in witness_histories(texts):
        req = {"op": op} if f is None else {"op": op, "file": f, "text": text}
        if f is not None:
            texts[f] = text
            open(os.path.join(ROOT, work, f), "w", encoding="utf-8").write(text)
        r, ms = d.ask(req)
        entry = {"op": op, "point": point, "file": f, "state": sha(json.dumps(texts, sort_keys=True)), "reply": r, "error": None if r else ms}
        if op in ("open", "edit"):
            entry["batch_ok"], entry["batch"], entry["batch_status"] = batch(fortisc, work)
        log.write(json.dumps(entry) + "\n")
        if r is None or r.get("ok") is not True:
            reason = "system-failure"
            break
    log.write(json.dumps({"op": "end", "reason": reason}) + "\n")
    log.close()
    d.close()


def cold(size, fortisc, out_dir):
    """Every cold execution's status is kept, and each must succeed (apparatus review 12)."""
    pkg = pkg_of(size)
    rows = {"batch_s": [], "batch_ok": [], "batch_status": [], "daemon_s": [], "daemon_ok": [], "daemon_error": []}
    for _ in range(5):
        t = time.perf_counter()
        try:
            r = subprocess.run([fortisc, "check", pkg], cwd=ROOT, capture_output=True, text=True, timeout=600)
            ok = r.returncode == 0 and not r.stdout.strip() and not r.stderr.strip()
            status = {"exit": r.returncode, "stdout": r.stdout[-300:], "stderr": r.stderr[-300:]}
        except subprocess.TimeoutExpired:
            ok, status = False, {"exit": "timeout"}
        rows["batch_s"].append(time.perf_counter() - t)
        rows["batch_ok"].append(ok)
        rows["batch_status"].append(status)
    for k in range(3):
        t = time.perf_counter()
        d = Daemon(fortisc, pkg, dict(os.environ), os.path.join(out_dir, f"cold-{k}.err"))
        r, ms = d.ask({"op": "open"})
        rows["daemon_s"].append(time.perf_counter() - t)
        rows["daemon_ok"].append(bool(r and r.get("ok") and isinstance(r.get("records"), list)))
        rows["daemon_error"].append(ms if r is None else r.get("error"))
        d.close()
    json.dump(rows, open(os.path.join(out_dir, "cold.json"), "w"), indent=1)
    return rows


def daemon_of(fortisc):
    """The daemon executable `fortisc daemon` runs: `fortisd`, beside it (X-141)."""
    return os.path.join(os.path.dirname(fortisc), "fortisd")


def run(size, out_dir, fortisc=FORTISC, env=None, mode="full", generate=True):
    os.makedirs(out_dir, exist_ok=True)
    env = dict(env or os.environ)
    workload = ensure_workload(size) if generate else "generated before the apparatus check"
    # what the batch answers must be a function of, fixed before anything is measured (re-review 4)
    info = {"workload": workload, "fortisc": fortisc, "fortisc_sha256": sha_file(fortisc), "fortisd": daemon_of(fortisc), "fortisd_sha256": sha_file(daemon_of(fortisc)), "identity": identity(size), "mode": mode, "size": size}
    if mode == "full":
        info["cold"] = cold(size, fortisc, out_dir)
    fixtures_session(fortisc, out_dir, env)
    witness_session(fortisc, out_dir, env)
    for name in RUNS:
        session(fortisc, size, mode, name, os.path.join(out_dir, f"{name}.jsonl"), env)
    json.dump(info, open(os.path.join(out_dir, "run.json"), "w"), indent=1)
    return True


# ---- the batch check of every state -------------------------------------------------------------------------------
def answers_path(size):
    return os.path.join(ROOT, "build", "incremental", f"answers-{size}.jsonl")


def load_answers(size, ident):
    out = {}
    p = answers_path(size)
    for part in [p] + [f"{p}.w{k}" for k in range(64)]:
        if os.path.exists(part):
            for line in open(part, encoding="utf-8"):
                try:
                    a = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if a.get("identity") == ident and a.get("ok"):
                    out[a["state"]] = a["lines"]
    return out


def worker(args):
    k, size, mode, want, ident = args
    pkg = pkg_of(size)
    wdir = os.path.join("build", "incremental", "verify", f"{size}-w{k}")
    if os.path.isdir(os.path.join(ROOT, wdir)):
        shutil.rmtree(os.path.join(ROOT, wdir))
    shutil.copytree(os.path.join(ROOT, pkg), os.path.join(ROOT, wdir))
    written = dict(States(pkg).texts)
    out = open(answers_path(size) + f".w{k}", "a")
    done, failures = set(), []

    def answer(st):
        dg = st.digest()
        if dg not in want or dg in done:
            return
        for f, t in st.texts.items():
            if written[f] != t:
                open(os.path.join(ROOT, wdir, f), "w", encoding="utf-8").write(t)
                written[f] = t
        ok, lines, status = batch(FORTISC, wdir)
        out.write(json.dumps({"state": dg, "identity": ident, "ok": ok, "lines": normalise(lines, wdir), "status": status}) + "\n")
        out.flush()
        done.add(dg)
        if not ok:
            failures.append((dg, status))

    for run_name in RUNS:
        st = States(pkg)
        answer(st)
        for _, e in stream(size, mode, run_name):
            st.set(e.file, e.text)
            answer(st)
    out.close()
    return k, len(done), failures


def logged_states(out_dir):
    seen = set()
    for name in RUNS:
        p = os.path.join(out_dir, f"{name}.jsonl")
        if not os.path.exists(p):
            continue
        for line in open(p, encoding="utf-8"):
            e = json.loads(line)
            if e.get("op") in ("open", "edit") and e.get("state"):
                seen.add(e["state"])
    return seen


def verify(size, out_dir, workers, mode="full"):
    ident = identity(size)
    have = load_answers(size, ident)
    todo = sorted(logged_states(out_dir) - set(have))
    print(f"verify {size}: {len(todo)} states to check ({len(have)} answered), {workers} workers", flush=True)
    t = time.time()
    if todo:
        parts = [set(todo[k::workers]) for k in range(workers)]
        with multiprocessing.get_context("fork").Pool(workers) as pool:
            for k, n, fails in pool.imap_unordered(worker, [(k, size, mode, parts[k], ident) for k in range(workers) if parts[k]]):
                print(f"  worker {k}: {n} states checked, {len(fails)} without a valid batch answer {fails[:2]}", flush=True)
    have = load_answers(size, ident)
    missing = len(logged_states(out_dir) - set(have))
    print(f"verify {size}: done in {time.time() - t:.0f} s; {missing} states without a batch answer", flush=True)
    return missing == 0


# ---- the checks -----------------------------------------------------------------------------------------------------
def pct(xs, p):
    xs = sorted(xs)
    if not xs:
        return None
    k = max(0, min(len(xs) - 1, int(round(p / 100.0 * len(xs) + 0.5)) - 1))
    return xs[k]


def summary(xs):
    return {"n": len(xs), "p50": pct(xs, 50), "p95": pct(xs, 95), "p99": pct(xs, 99), "max": max(xs) if xs else None, "mean": statistics.mean(xs) if xs else None}


def read_log(p):
    return [json.loads(l) for l in open(p, encoding="utf-8")] if os.path.exists(p) else []


def slope(points):
    """The least-squares slope of (x, y) points."""
    n = len(points)
    mx = sum(x for x, _ in points) / n
    my = sum(y for _, y in points) / n
    den = sum((x - mx) ** 2 for x, _ in points)
    # fewer than two distinct cycle ends is no trend at all: it fails rather than passing as flat (closing review 5)
    return sum((x - mx) * (y - my) for x, y in points) / den if den else float("inf")


def expected_states(size, mode, run):
    """The states a run must reach, in order, regenerated from the scripts (apparatus review 6)."""
    st = States(pkg_of(size))
    out = [st.digest()]
    for _, e in stream(size, mode, run):
        st.set(e.file, e.text)
        out.append(st.digest())
    return out


def expected_requests(size, mode, run):
    """Every request a run must log, in order, as `session` makes them (SP-12b apparatus re-review 1): `open`, each
    edit with the state it reaches, each `stats` with its label and cycle, and each deep comparison with its step
    and state."""
    st = States(pkg_of(size))
    out = [("open", st.digest()), ("stats", "open", None)]
    items = list(stream(size, mode, run))
    deep = set(deep_points(run, mode, items))
    last = 0
    for n, (c, e) in enumerate(items):
        if c != last:
            out.append(("stats", f"cycle {last}", last))
            last = c
        st.set(e.file, e.text)
        out.append(("edit", n, st.digest()))
        if run == "latency" and (n + 1) % 100 == 0:
            out.append(("stats", f"edit {n + 1}", None))
        if n in deep:
            out.append(("verify", n, st.digest()))
    if run == "soak":
        out.append(("stats", f"cycle {last}", last))
    return out + [("stats", "end", None)]


def logged_requests(log):
    """A log's requests as `expected_requests` names them."""
    out = []
    for e in log:
        if e["op"] == "open":
            out.append(("open", e.get("state")))
        elif e["op"] == "stats":
            out.append(("stats", e.get("after"), e.get("cycle")))
        elif e["op"] in ("edit", "verify"):
            out.append((e["op"], e.get("step"), e.get("state")))
    return out


def report(size, out_dir, mode="full"):
    pkg = pkg_of(size)
    ident = identity(size)
    answers = load_answers(size, ident)
    orc = oracle.Oracle(os.path.join(ROOT, pkg))
    checks, figures, missing = {}, {}, []

    def check(name, ok, detail=None):
        checks[name] = {"pass": bool(ok), "detail": detail}

    info = json.load(open(os.path.join(out_dir, "run.json"))) if os.path.exists(os.path.join(out_dir, "run.json")) else {}
    # the answers are those of the measured run's identity, and the daemon measured is the executable named
    if info.get("identity") != ident:
        missing.append("the batch answers' identity is not the one recorded before the run")
    if not info.get("fortisc") or not os.path.exists(info["fortisc"]) or sha_file(info["fortisc"]) != info.get("fortisc_sha256"):
        missing.append("the batch check's executable is not the one the run recorded")
    if not info.get("fortisd") or not os.path.exists(info["fortisd"]) or sha_file(info["fortisd"]) != info.get("fortisd_sha256"):
        missing.append("the daemon's executable is not the one the run recorded")
    logs = {n: read_log(os.path.join(out_dir, f"{n}.jsonl")) for n in RUNS}
    failed_runs = set()
    # P: each run reached its end, and the log holds the preregistered requests
    for name, log in logs.items():
        ends = [e for e in log if e["op"] == "end"]
        if not ends:
            missing.append(f"{name}: no log, or no terminal reason")
            continue
        reason = ends[-1]["reason"]
        # the run made exactly the preregistered requests, or, when it failed, exactly their prefix up to the request
        # that failed: only the unattempted rest is waived (SP-12b apparatus re-review 1)
        got, want = logged_requests(log), expected_requests(size, mode, name)
        if reason == "completed" and got != want:
            missing.append(f"{name}: {len(got)} requests logged, {len(want)} expected, or not the script's")
        elif reason != "completed":
            failed_runs.add(name)
            if not got or got != want[: len(got)]:
                missing.append(f"{name}: the requests logged up to the failure are not the script's")
        bad = [e.get("step") for e in log if e["op"] in ("open", "edit") and not (e.get("reply") or {}).get("ok")]
        bad += [e.get("after") for e in log if e["op"] == "stats" and (e.get("reply") or {}).get("ok") is not True]
        # every request is in P, the deep comparisons too (closing review 6)
        bad += [e.get("step") for e in log if e["op"] == "verify" and (e.get("reply") or {}).get("ok") is not True]
        check(f"P.{name}", reason == "completed" and not bad, {"reason": reason, "detail": ends[-1].get("detail"), "failed": bad[:5]})
    # R: every reply, against the batch answer of its state; the digest the reply states, against its lines
    for name, log in logs.items():
        r_bad, d_bad, unanswered, n = [], [], 0, 0
        for e in log:
            if e["op"] not in ("open", "edit"):
                continue
            # every state a run reached has a batch answer, a failed request's too (SP-12b apparatus review 3)
            want = answers.get(e["state"])
            if want is None:
                unanswered += 1
            if not (e.get("reply") or {}).get("ok"):
                continue
            n += 1
            rep = e["reply"]
            lines = rep["records"]
            if not isinstance(lines, list) or sha("\n".join(lines)) != rep.get("digest"):
                d_bad.append(e.get("step"))
            if want is not None and normalise(lines, pkg) != want:
                r_bad.append({"step": e.get("step"), "edit": e.get("edit"), "got": len(lines), "want": len(want)})
        if unanswered:
            missing.append(f"{name}: {unanswered} replies whose state has no batch answer")
        check(f"R.{name}", not r_bad, {"mismatches": len(r_bad), "first": r_bad[:3], "replies": n})
        check(f"R.digest.{name}", not d_bad, {"mismatches": d_bad[:5]})
    fx = read_log(os.path.join(out_dir, "fixtures.jsonl"))
    if not fx or fx[-1].get("op") != "end":
        missing.append("fixtures: no log")
    else:
        steps = json.load(open(os.path.join(ROOT, FIXTURES, "steps.json"), encoding="utf-8"))
        bad, invalid = [], []
        for name in (k for k in steps if k != "about"):
            mine = [e for e in fx if e.get("fixture") == name]
            if len(mine) != 1 + len(steps[name]):
                bad.append((name, "replies", len(mine)))
            for e in mine:
                if not e.get("batch_ok"):
                    invalid.append((name, e.get("step"), e.get("batch_status")))
                elif not (e.get("reply") or {}).get("ok") or e["reply"]["records"] != e["batch"]:
                    bad.append((name, e.get("step")))
        if invalid:
            missing.append(f"fixtures: batch checks without a valid answer {invalid[:3]}")
        check("R.fixtures", not bad, {"mismatches": bad[:6]})
    # G2's witnesses: the daemon back at the fixture's original texts after each history has the counts it had after
    # `open` (SP-12b apparatus review 4). The log must hold exactly the witness's requests (a prefix, up to the one
    # that failed, when one failed); every reply equals the batch check of its state (R.witness)
    wl = read_log(os.path.join(out_dir, "witness.jsonl"))
    if not wl or wl[-1].get("op") != "end":
        missing.append("witness: no log")
    else:
        work = os.path.join(ROOT, FIXTURES, WITNESS)
        texts = {f"src/{f}": open(os.path.join(work, "src", f), encoding="utf-8").read() for f in ("a.ft", "b.ft")}
        want = []
        for op, point, f, text in witness_histories(dict(texts)):
            if f is not None:
                texts[f] = text
            want.append((op, point, f, sha(json.dumps(texts, sort_keys=True))))
        got = [(e["op"], e.get("point"), e.get("file"), e.get("state")) for e in wl if e["op"] != "end"]
        reason = wl[-1]["reason"]
        if (reason == "completed" and got != want) or (reason != "completed" and (not got or got != want[: len(got)])):
            missing.append("witness: the requests logged are not the witness's")
        check("P.witness", reason == "completed", {"reason": reason})
        r_bad, invalid = [], []
        for e in wl:
            if e["op"] in ("open", "edit"):
                if not e.get("batch_ok"):
                    invalid.append((e["point"], e.get("batch_status")))
                elif (e.get("reply") or {}).get("records") != e.get("batch"):
                    r_bad.append(e["point"])
        if invalid:
            missing.append(f"witness: batch checks without a valid answer {invalid[:3]}")
        check("R.witness", not r_bad, {"mismatches": r_bad[:4]})
        if reason == "completed":
            points = {e["point"]: e["reply"] for e in wl if e["op"] == "stats"}
            got_counts = {p: [points[p].get(k) for k in COUNTS] for p in points}
            if any(v is None for c in got_counts.values() for v in c):
                missing.append("witness: a statistics reply without the allocator counts")
            else:
                check("G2.witness", got_counts["tables"] == got_counts["open"] and got_counts["scratch"] == got_counts["open"], got_counts)
    # V: the replay's deep comparisons (apparatus review 9), at exactly the points the scripts give (re-review 1)
    for name in ("replay-latency", "replay-soak"):
        vs = [e for e in logs[name] if e["op"] == "verify"]
        # a run that failed before its first deep point made none: V is what its failure prevented
        if name in failed_runs and not vs:
            continue
        fails = []
        for e in vs:
            r = e.get("reply") or {}
            if r.get("ok") is not True or not isinstance(r.get("differences"), list) or r["differences"]:
                fails.append((e["step"], r.get("differences") or r.get("error") or e.get("error")))
        check(f"V.{name}", vs and not fails, {"points": len(vs), "failing": fails[:3]})
    # I: the measured latency run's invalidation
    lat = [e for e in logs["latency"] if e["op"] == "edit" and (e.get("reply") or {}).get("ok")]
    i_bad = []
    for e in lat:
        must, may = orc.sets(e["edit"])
        ok, miss, extra = orc.check(must, may, [tuple(p) for p in e["reply"]["ran"]["passes"]])
        if not ok:
            i_bad.append({"step": e["step"], "kind": e["edit"]["kind"], "role": e["edit"]["role"], "missing": miss, "extra": extra})
    check("I.latency", lat and not i_bad, {"edits": len(lat), "failing": len(i_bad), "first": i_bad[:3]})
    # latency figures, by kind and class (the warm-up excluded)
    measured = [e for e in lat if e["step"] >= WARMUP]
    by_kind = {}
    for e in measured:
        k = e["edit"]["kind"] if e["edit"]["role"] in ("open", "restore") else f"{e['edit']['kind']}-{e['edit']['role']}"
        by_kind.setdefault(k, []).append(e["wall_ms"])
    figures["latency_ms_by_kind"] = {k: summary(v) for k, v in sorted(by_kind.items())}
    body = [e["wall_ms"] for e in measured if e["edit"]["kind"] in BODY]
    sig = [e for e in measured if e["edit"]["kind"] in SIGNATURE or (e["edit"]["kind"] in ("probe", "aspect") and e["edit"]["role"] in ("open", "restore"))]
    figures["body_ms"] = summary(body)
    figures["signature_ms"] = summary([e["wall_ms"] for e in sig])
    figures["daemon_us_body"] = summary([e["reply"]["us"] for e in measured if e["edit"]["kind"] in BODY])
    hubs = {}
    for e in sig:
        if "hub" in e["edit"]:
            hubs.setdefault(e["edit"]["hub"], []).append(e["wall_ms"])
    figures["signature_ms_by_hub"] = {h: summary(v) for h, v in sorted(hubs.items())}
    passes = {}
    for e in measured:
        passes.setdefault(e["edit"]["kind"], []).append((len(e["reply"]["ran"]["passes"]), e["reply"]["ran"]["prepared"]))
    figures["invalidation_by_kind"] = {k: {"passes_mean": statistics.mean(p for p, _ in v), "passes_max": max(p for p, _ in v), "contexts_mean": statistics.mean(c for _, c in v)} for k, v in sorted(passes.items())}
    if size == "50k":
        check("L1.50k", body and pct(body, 95) <= 100.0, {"p95_ms": pct(body, 95), "n": len(body)})
        worst = max((e["wall_ms"] for e in sig), default=None)
        check("L2.50k", sig and worst <= 500.0, {"max_ms": worst, "n": len(sig), "over": [(e["step"], e["edit"]["kind"], round(e["wall_ms"], 1)) for e in sig if e["wall_ms"] > 500.0][:5]})
    # C: the cold checks. A cold daemon that fails is a failure of the system (P.cold); a cold batch check that fails
    # is evidence that could not be collected (re-review 5); the budget is judged on valid executions only
    if mode == "full":
        c = info.get("cold")
        if not c or len(c["batch_s"]) != 5 or len(c["daemon_s"]) != 3:
            missing.append("cold: not every cold run was made")
        else:
            figures["cold"] = {"batch_s": c["batch_s"], "daemon_s": c["daemon_s"]}
            check("P.cold", all(c["daemon_ok"]), {"errors": c.get("daemon_error")})
            if not all(c["batch_ok"]):
                missing.append(f"cold: a batch check failed {c.get('batch_status')}")
            else:
                check(f"C.batch.{size}", statistics.median(c["batch_s"]) <= COLD[size], {"median_s": statistics.median(c["batch_s"])})
            if all(c["daemon_ok"]):
                check(f"C.daemon.{size}", statistics.median(c["daemon_s"]) <= COLD[size], {"median_s": statistics.median(c["daemon_s"])})
    # M, G: memory, from the measured processes only
    # the measurements come from successful statistics replies only; a failed one is P's (re-review 5)
    stats = {n: [e for e in logs[n] if e["op"] == "stats" and (e.get("reply") or {}).get("ok") is True] for n in ("latency", "soak")}
    hwm = max((s["reply"]["hwm_kib"] for n in stats for s in stats[n]), default=None)
    figures["hwm_kib"] = hwm
    figures["memory_latency"] = [(s["after"], s["reply"]["rss_kib"], s["reply"]["heap_bytes"], s["reply"]["memos"]) for s in stats["latency"]]
    check(f"M.{size}", hwm is not None and hwm * 1024 <= HWM[size], {"hwm_mib": hwm / 1024 if hwm else None})
    if "soak" not in failed_runs:
        opened = next((s["reply"] for s in stats["soak"] if s["after"] == "open"), None)
        ends = [s for s in stats["soak"] if "cycle" in s]
        figures["soak_cycle_ends"] = [{"cycle": s["cycle"], "memos": s["reply"]["memos"], "heap_bytes": s["reply"]["heap_bytes"], **{k: s["reply"].get(k) for k in ("live_bytes", "live_allocs", "in_use_bytes", "footprint_bytes", "mapped_bytes")}, "rss_kib": s["reply"]["rss_kib"], "hwm_kib": s["reply"]["hwm_kib"]} for s in ends]
        if not opened or len(ends) != CYCLES[mode]:
            missing.append(f"soak: {len(ends)} cycle ends, {CYCLES[mode]} expected")
        else:
            check(f"G1.{size}", all(s["reply"]["memos"] == opened["memos"] for s in ends), {"after_open": opened["memos"], "cycle_ends": [s["reply"]["memos"] for s in ends]})
            # G2, retention (X-141): the live bytes the daemon requested, its live allocations, and the bytes its
            # size-class allocator holds for them are a function of the state, so the daemon after `open` and at every
            # cycle end (all at the package's original texts) has the same three counts, exactly
            later = ends[1:]
            if [s["cycle"] for s in ends] != list(range(CYCLES[mode])):
                missing.append(f"soak: the cycle ends are {[s['cycle'] for s in ends]}, not 0..{CYCLES[mode] - 1}")
            keys = ("live_bytes", "live_allocs", "in_use_bytes")
            counts = [tuple(s.get(k) for k in keys) for s in [opened] + [s["reply"] for s in ends]]
            if any(v is None or v <= 0 for c in counts for v in c) or any(s["reply"].get("footprint_bytes") is None for s in ends):
                missing.append("soak: a statistics reply without the daemon's allocator counts")
            else:
                spread = [max(c[k] for c in counts) - min(c[k] for c in counts) for k in range(3)]
                figures["live_spread"] = dict(zip(keys, spread))
                check(f"G2.{size}", all(spread[k] <= LIVE_TOLERANCE[k] for k in range(3)), {"spread": dict(zip(keys, spread)), "tolerance": LIVE_TOLERANCE, "points": len(counts)})
                # G4, the allocator's own growth, a budget: its footprint (each class's peak, so it moves with the
                # rarest transient) may grow from cycle end 1 to the last by at most 8 MiB plus 0.5% (X-141, SP-12b
                # design 3)
                f1, fn = later[0]["reply"]["footprint_bytes"], later[-1]["reply"]["footprint_bytes"]
                allowance = G4_ALLOWANCE + G4_SHARE * f1
                figures["footprint_growth_bytes"] = fn - f1
                figures["footprint_slope_bytes_per_cycle"] = slope([(s["cycle"], s["reply"]["footprint_bytes"]) for s in later])
                check(f"G4.{size}", fn - f1 <= allowance, {"growth_bytes": fn - f1, "allowance_bytes": round(allowance)})
            figures["heap_slope_bytes_per_cycle"] = slope([(s["cycle"], s["reply"]["heap_bytes"]) for s in later])
            # the resident set: the allocator returns freed memory now and then, so a cycle end can dip by tens of
            # megabytes but never rise without growth; the peak of the later half of the cycle ends is compared with
            # the peak of the earlier half
            rss = [s["reply"]["rss_kib"] * 1024 for s in ends]
            half = len(rss) // 2
            growth = max(rss[half:]) - max(rss[:half])
            figures["rss_peak_growth_bytes"] = growth
            check(f"G3.{size}", growth <= G3_ALLOWANCE + 0.02 * max(rss[:half]), {"peak_growth_bytes": growth, "rss_kib_at_ends": [s["reply"]["rss_kib"] for s in ends]})
    # a failed P check outranks the measurements its failure prevented: V and the soak's memory checks are skipped
    # for the run that failed, where they arise; nothing else is waived (SP-12b apparatus review 3)
    return {"size": size, "mode": mode, "complete": not missing, "missing": missing, "checks": checks, "figures": figures, "run": info, "identity": ident}


def families(res):
    return {name.split(".")[0] for name, c in res["checks"].items() if not c["pass"]}


def one_mutation(out_dir, name, fault, workers):
    d = os.path.join(out_dir, name)
    env = dict(os.environ)
    env.pop("FORTIS_QDB_FAULT", None)
    if fault:
        env["FORTIS_QDB_FAULT"] = fault
    t = time.time()
    run("50k", d, fortisc=FAULT_FORTISC, env=env, mode="reduced", generate=False)
    verify("50k", d, workers, mode="reduced")
    res = report("50k", d, mode="reduced")
    json.dump(res, open(os.path.join(d, "results.json"), "w"), indent=1, sort_keys=True)
    fails = families(res)
    if not res["complete"]:
        status = "incomplete" if fault is None else "apparatus-failure"
    elif fault is None:
        status = "pass" if not fails else "fail"
    else:
        status = "killed" if fails & FAULTS[fault] else "SURVIVED"
    return name, {"status": status, "complete": res["complete"], "missing": res["missing"], "failed_families": sorted(fails), "expected": sorted(FAULTS[fault]) if fault else [], "checks_failed": [k for k, c in res["checks"].items() if not c["pass"]], "seconds": round(time.time() - t)}


def mutations(out_dir, json_out, workers, parallel=6):
    """The apparatus check (design 10): the reduced run on the fault build, with no fault first, then every fault, six at
    a time (their states are the control's, so their batch answers are cached), and last, alone, the faults a timing
    check must kill."""
    os.makedirs(out_dir, exist_ok=True)
    # the workload is generated once, before any run, and no run regenerates it (re-review 3)
    print(ensure_workload("50k"), flush=True)
    results = {}

    def save():
        json.dump({"complete": len(results) == 1 + len(FAULTS), "control": results.get("control"), "results": results}, open(json_out, "w"), indent=1)
    name, res = one_mutation(out_dir, "control", None, workers)
    results[name] = res
    print(f"control: {res['status']} (failed: {', '.join(res['failed_families']) or 'none'}; missing: {res['missing']}) in {res['seconds']} s", flush=True)
    save()
    # a fault that a timing check must kill runs alone, so that what kills it is the fault, not the other runs
    timed = sorted(f for f in FAULTS if FAULTS[f] & {"L1", "L2"})
    with ThreadPoolExecutor(parallel) as ex:
        for name, res in ex.map(lambda f: one_mutation(out_dir, f, f, 2), sorted(set(FAULTS) - set(timed))):
            results[name] = res
            print(f"{name}: {res['status']} (failed: {', '.join(res['failed_families']) or 'none'}) in {res['seconds']} s", flush=True)
            save()
    for f in timed:
        name, res = one_mutation(out_dir, f, f, workers)
        results[name] = res
        print(f"{name}: {res['status']} (failed: {', '.join(res['failed_families']) or 'none'}) in {res['seconds']} s", flush=True)
        save()
    return results


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["run", "verify", "report", "mutations"])
    ap.add_argument("--size", choices=list(SIZES), default="50k")
    ap.add_argument("--out", required=True)
    ap.add_argument("--json")
    ap.add_argument("--workers", type=int, default=12)
    ap.add_argument("--mode", choices=["full", "reduced"], default="full")
    a = ap.parse_args(argv)
    if a.cmd == "run":
        return 0 if run(a.size, a.out, mode=a.mode) else 1
    if a.cmd == "verify":
        return 0 if verify(a.size, a.out, a.workers, mode=a.mode) else 1
    if a.cmd == "report":
        res = report(a.size, a.out, mode=a.mode)
        if a.json:
            json.dump(res, open(a.json, "w"), indent=1, sort_keys=True)
        for k, c in sorted(res["checks"].items()):
            print(f"{'PASS' if c['pass'] else 'FAIL'} {k} {json.dumps(c['detail'])[:300]}")
        print(f"complete: {res['complete']} {res['missing']}")
        return 0 if res["complete"] and all(c["pass"] for c in res["checks"].values()) else 1
    if a.cmd == "mutations":
        res = mutations(a.out, a.json or os.path.join(a.out, "mutations.json"), a.workers)
        return 0 if all(r["status"] in ("pass", "killed") for r in res.values()) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
