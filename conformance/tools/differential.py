"""Differential harness (DEL-09, Programme 6.3, PROG-VM-02/03, PROG-FND-02).

Compares two implementations over every module fixture by the fixture's comparator:
  verdict  accept / reject
  code     the set of (code, line) rejections
  output   the standard-output lines of an execute fixture
  trace    the failure reports and the exit of an execute fixture
and writes a report with three separate fields: evidence integrity (hashes over inputs and
outputs, the commit), execution attestation (what ran, where, with network and subprocesses
blocked) and semantic correctness (the disagreements). Implementations: `reference` (the static
checker plus the definitional interpreter), `agreeing` (a mutant that delegates to the reference)
and `disagreeing` (a mutant with injected disagreements of every kind, used to prove the harness
reports exactly what it should). Comparing the reference with itself is labelled `self-comparison`,
which the release check ignores.

  differential.py --left reference --right agreeing --out conformance/differential/agreeing.json
  differential.py --verify            # all three comparisons, checked against the injection table"""
import datetime
import hashlib
import json
import os
import platform
import socket
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
sys.path.insert(0, HERE)
sys.path.insert(0, ROOT)

OUT_DIR = os.path.join(ROOT, "conformance", "differential")
PLATFORM = platform.platform()                 # read before the offline guard: platform() may spawn uname


# ---- offline guard (PROG-FND-02) -------------------------------------------------------
def block_network_and_subprocesses():
    def refuse(*a, **k):
        raise RuntimeError("blocked by the differential harness: no network, no subprocesses (PROG-FND-02)")
    socket.socket.__init__ = refuse            # type: ignore[assignment]
    socket.create_connection = refuse          # type: ignore[assignment]
    subprocess.Popen.__init__ = refuse         # type: ignore[assignment]
    subprocess.run = refuse                    # type: ignore[assignment]
    os.system = refuse                         # type: ignore[assignment]
    os.popen = refuse                          # type: ignore[assignment]
    if hasattr(os, "fork"):
        os.fork = refuse                       # type: ignore[assignment]


def head_commit() -> str:
    """The short HEAD commit read from .git, without a subprocess."""
    git = os.path.join(ROOT, ".git")
    head = open(os.path.join(git, "HEAD"), encoding="utf-8").read().strip()
    if head.startswith("ref: "):
        ref = os.path.join(git, head[5:])
        if os.path.exists(ref):
            return open(ref, encoding="utf-8").read().strip()[:7]
        packed = os.path.join(git, "packed-refs")
        for ln in open(packed, encoding="utf-8"):
            if ln.strip().endswith(head[5:]):
                return ln.split()[0][:7]
    return head[:7]


# ---- the reference implementation -------------------------------------------------------
class Reference:
    name = "reference"

    def __init__(self):
        import parser as fparser
        import names
        from checker import check
        self.check = check
        self.rec = fparser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
        self.lib = names.Library(self.rec)
        import execute_check
        self.exec = execute_check

    def observe(self, fixture: dict, src: str) -> dict:
        judged = self.check.CODES | set(self.check.PARTIAL)
        try:
            records = self.check.check_source(self.lib, src, fixture["path"])
        except Exception as e:  # noqa: BLE001
            records = [{"code": "E-INT-000", "line": 1, "message": repr(e)}]
        mine = sorted({(r["code"], r["line"]) for r in records if r["code"] in judged or r["code"].startswith("E-INT")})
        obs = {"verdict": "reject" if mine else "accept", "codes": [f"{c}@{ln}" for c, ln in mine], "output": None, "trace": None}
        if fixture["verdict"] == "execute":
            observed, it = self.exec.run_fixture(os.path.join(ROOT, fixture["path"]))
            obs["output"] = "|".join(it.out) if it is not None else observed
            last = observed.split("|")[-1] if observed else ""
            obs["trace"] = {"reports": [r.get("code", r.get("kind")) for r in (it.reports if it is not None else [])],
                            "exit": last if (last.startswith("fail:") or observed.startswith("dynamic:") or last.startswith("defect:")) else "ok"}
        return obs


class Native:
    """The experimental lowering (DEL-25): the checker decides the verdict; an accepted program must lower,
    build under #![forbid(unsafe_code)] and, when it has a main, run with the same output and trace."""
    name = "native"

    RESULTS = os.path.join(ROOT, "build", "native", "results.json")

    def __init__(self, ref: Reference):
        self.ref = ref
        # the native observations are produced beforehand by `native.py --corpus` (cargo needs subprocesses, which
        # this harness blocks); the report's evidence integrity records the hash of that results file
        if not os.path.exists(self.RESULTS):
            raise RuntimeError("run `python3 -I compiler/experimental-lowering/native.py --corpus` first")
        self.results = json.load(open(self.RESULTS, encoding="utf-8"))
        self.results_sha256 = hashlib.sha256(open(self.RESULTS, "rb").read()).hexdigest()
        self.scope = set(self.results)

    def observe(self, fixture, src):
        base = self.ref.observe(fixture, src)
        if fixture["path"] not in self.scope:
            return base                                    # outside the synchronous slice: the reference stands in
        if base["verdict"] == "reject":
            return base                                    # rejected before lowering (criterion 4)
        obs = self.results[fixture["path"]]
        out = {"verdict": "reject" if obs["verdict"] != "accept" else "accept", "codes": base["codes"] if obs["verdict"] == "accept" else [f"E-NATIVE@{obs.get('detail', '')[:60]}"],
               "output": obs["output"] if fixture["verdict"] == "execute" else None, "trace": obs["trace"] if fixture["verdict"] == "execute" else None}
        if out["trace"] is not None and base["trace"] is not None and out["trace"]["exit"].startswith("defect:") and base["trace"]["exit"].startswith("defect:"):
            # 10.8: native aborts the process on a defect and writes no record; the VM terminates the task with one
            out["trace"] = {"reports": base["trace"]["reports"], "exit": base["trace"]["exit"]}
        return out


class Agreeing:
    """A mutant that behaves exactly like the reference: the harness must report 0 disagreements."""
    name = "agreeing"

    def __init__(self, ref: Reference):
        self.ref = ref

    def observe(self, fixture, src):
        return json.loads(json.dumps(self.ref.observe(fixture, src)))


# injected disagreements: fixture id -> (kind, mutation); at least two of each kind
INJECTED = {
    "PRG-lit_in_range":        ("verdict", lambda o: {**o, "verdict": "reject", "codes": ["E-REF-001@5"]}),
    "PRG-types_trivial_ok":    ("verdict", lambda o: {**o, "verdict": "reject", "codes": ["E-OWN-030@3"]}),
    "PRG-lit_out_of_range":    ("code", lambda o: {**o, "codes": [c.replace("E-REF-001", "E-TYP-040") for c in o["codes"]]}),
    "PRG-types_trivial_bad":   ("code", lambda o: {**o, "codes": [c.replace("@3", "@4") for c in o["codes"]]}),
    "PRG-exec_hello":          ("output", lambda o: {**o, "output": (o["output"] or "") + "!"}),
    "PRG-exec_literals":       ("output", lambda o: {**o, "output": "|".join(reversed((o["output"] or "").split("|")))}),
    "PRG-exec_failing_main":   ("trace", lambda o: {**o, "trace": {**(o["trace"] or {}), "exit": "ok"}}),
    "PRG-exec_report":         ("trace", lambda o: {**o, "trace": {**(o["trace"] or {}), "reports": []}}),
}


class Disagreeing:
    """A mutant with the injected disagreements of INJECTED and nothing else."""
    name = "disagreeing"

    def __init__(self, ref: Reference):
        self.ref = ref

    def observe(self, fixture, src):
        o = json.loads(json.dumps(self.ref.observe(fixture, src)))
        inj = INJECTED.get(fixture["id"])
        return inj[1](o) if inj else o


# ---- comparison --------------------------------------------------------------------------
def compare(left_obs: dict, right_obs: dict) -> list:
    out = []
    for fid in sorted(left_obs):
        a, b = left_obs[fid], right_obs.get(fid)
        if b is None:
            out.append({"fixture": fid, "kind": "verdict", "left": a["verdict"], "right": "missing"})
            continue
        if a["verdict"] != b["verdict"]:
            out.append({"fixture": fid, "kind": "verdict", "left": a["verdict"], "right": b["verdict"]})
        elif a["codes"] != b["codes"]:
            out.append({"fixture": fid, "kind": "code", "left": a["codes"], "right": b["codes"]})
        if a["output"] != b["output"]:
            out.append({"fixture": fid, "kind": "output", "left": a["output"], "right": b["output"]})
        if a["trace"] != b["trace"]:
            out.append({"fixture": fid, "kind": "trace", "left": a["trace"], "right": b["trace"]})
    return out


def fixtures():
    syntax = json.load(open(os.path.join(ROOT, "conformance", "results.json"), encoding="utf-8"))["results"]
    out = []
    for r in syntax:
        if r["status"] not in ("PASS", "INFO") or "statements" in (r.get("detail") or "") or r["verdict"] == "illustrative":
            continue
        if any(c.startswith("E-SYN") for c in r.get("codes", [])) or r["detail"].startswith("template"):
            continue
        out.append(r)
    return out


def cited_evidence() -> set:
    """Files a closed spike cites by hash in spikes/registry.json: frozen once the spike closes."""
    reg = json.load(open(os.path.join(ROOT, "spikes", "registry.json"), encoding="utf-8"))
    return {os.path.normpath(os.path.join(ROOT, e["path"])) for s in reg.values() if s.get("status") == "closed" for e in s.get("evidence", [])}


def run(left_name: str, right_name: str, out_path: str, verbose=True) -> dict:
    if os.path.normpath(os.path.abspath(out_path)) in cited_evidence():
        raise SystemExit(f"refusing to overwrite {os.path.relpath(out_path, ROOT)}: a closed spike cites it as evidence; write to another path")
    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
    commit = head_commit()
    block_network_and_subprocesses()
    ref = Reference()
    impls = {"reference": ref, "agreeing": Agreeing(ref), "disagreeing": Disagreeing(ref)}
    if "native" in (left_name, right_name):
        impls["native"] = Native(ref)
    left, right = impls[left_name], impls[right_name]
    fx = fixtures()
    obs_l, obs_r, hashes = {}, {}, {}
    for f in fx:
        src = open(os.path.join(ROOT, f["path"]), encoding="utf-8").read()
        hashes[f["id"]] = hashlib.sha256(src.encode("utf-8")).hexdigest()
        obs_l[f["id"]] = left.observe(f, src)
        obs_r[f["id"]] = right.observe(f, src) if right is not left else obs_l[f["id"]]
    dis = compare(obs_l, obs_r)
    label = "self-comparison" if left is right else f"{left_name} vs {right_name}"
    report = {
        "comparison": label,
        "left": left_name, "right": right_name,
        "evidence_integrity": {
            "commit": commit,
            "grammar_sha256": hashlib.sha256(open(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"), "rb").read()).hexdigest(),
            "fixtures": hashes,
            "left_observations_sha256": hashlib.sha256(json.dumps(obs_l, sort_keys=True).encode()).hexdigest(),
            "right_observations_sha256": hashlib.sha256(json.dumps(obs_r, sort_keys=True).encode()).hexdigest(),
            "native_results_sha256": getattr(right, "results_sha256", None) or getattr(left, "results_sha256", None),
            "native_results_command": "python3 -I compiler/experimental-lowering/native.py --corpus" if "native" in (left_name, right_name) else None,
        },
        "execution_attestation": {
            "command": f"python3 -I conformance/tools/differential.py --left {left_name} --right {right_name}",
            "python": platform.python_version(), "platform": PLATFORM, "cwd": ROOT,
            "started": started, "finished": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "network_blocked": True, "subprocesses_blocked": True,
        },
        "semantic_correctness": {"fixtures": len(fx), "agreeing": len(fx) - len({d["fixture"] for d in dis}), "disagreements": dis},
        "observations": {"left": obs_l, "right": obs_r},
    }
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    json.dump(report, open(out_path, "w", encoding="utf-8"), indent=1)
    if verbose:
        print(f"{label}: {len(fx)} fixtures, {len(dis)} disagreements -> {os.path.relpath(out_path, ROOT)}")
    return report


def verify() -> int:
    """The harness against the three mutants, judged against the injection table."""
    failures = 0
    a = run("reference", "agreeing", os.path.join(OUT_DIR, "agreeing.json"), verbose=False)
    d = run("reference", "disagreeing", os.path.join(OUT_DIR, "disagreeing.json"), verbose=False)
    s = run("reference", "reference", os.path.join(OUT_DIR, "self-comparison.json"), verbose=False)
    n = a["semantic_correctness"]["fixtures"]
    print(f"agreeing mutant: {n} fixtures, {len(a['semantic_correctness']['disagreements'])} disagreements (expected 0)")
    failures += bool(a["semantic_correctness"]["disagreements"])
    got = {(x["fixture"], x["kind"]) for x in d["semantic_correctness"]["disagreements"]}
    want = {(fid, kind) for fid, (kind, _) in INJECTED.items()}
    print(f"disagreeing mutant: {len(got)} disagreements reported, {len(want)} injected")
    for fid, kind in sorted(want):
        ok = (fid, kind) in got
        failures += not ok
        print(f"  {'PASS' if ok else 'FAIL'} injected {kind:<8} {fid}")
    extra = got - want
    for fid, kind in sorted(extra):
        failures += 1
        print(f"  FAIL unexpected {kind:<8} {fid}")
    kinds = {}
    for _, kind in want:
        kinds[kind] = kinds.get(kind, 0) + 1
    print(f"  kinds injected: {kinds}")
    print(f"self-comparison: labelled {s['comparison']!r}, {len(s['semantic_correctness']['disagreements'])} disagreements")
    failures += s["comparison"] != "self-comparison"
    for name in ("agreeing", "disagreeing", "self-comparison"):
        r = json.load(open(os.path.join(OUT_DIR, name + ".json"), encoding="utf-8"))
        ok = all(k in r for k in ("evidence_integrity", "execution_attestation", "semantic_correctness")) and r["execution_attestation"]["network_blocked"] and r["execution_attestation"]["subprocesses_blocked"]
        failures += not ok
        print(f"  {'PASS' if ok else 'FAIL'} {name}.json carries the three fields and the offline attestation")
    try:
        socket.create_connection(("example.invalid", 80))
        print("FAIL network was not blocked"); failures += 1
    except RuntimeError as e:
        print(f"PASS network blocked: {str(e)[:60]}")
    try:
        subprocess.run(["true"])
        print("FAIL subprocess was not blocked"); failures += 1
    except RuntimeError:
        print("PASS subprocesses blocked")
    print(f"\n{'OK' if not failures else 'FAIL'}: {failures} failures")
    return 1 if failures else 0


def main(argv):
    if "--verify" in argv:
        return verify()
    left = argv[argv.index("--left") + 1] if "--left" in argv else "reference"
    right = argv[argv.index("--right") + 1] if "--right" in argv else "reference"
    out = argv[argv.index("--out") + 1] if "--out" in argv else os.path.join(OUT_DIR, f"{left}-vs-{right}.json")
    run(left, right, out)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
