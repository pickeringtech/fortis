"""DEL-29 runner: the canonical codec against its committed golden files (Language Reference 6.10, 6.6).

  1. Each case program in conformance/codec/cases/ checks with 0 diagnostics.
  2. builtins.ft and derived.ft run on the interpreter with the development FileSystem rooted at a fresh
     directory; each writes <name>.json and <name>.bin for every value and decodes it back from both, printing
     "<name> true true". Every written file must equal conformance/codec/golden/<case>/<file> byte for byte,
     and every golden file must be written (none stale). Since the program decodes the bytes it wrote, a match
     also shows the golden files decode to the original value.
  3. Independently of the reference codec, every written .json re-serialises through Python's `json` module
     (no whitespace, the same escapes) to the same text, and every .bin walks to its end under a schemaless
     reader of the FORMAT.md tags with minimal LEB128 throughout.
  4. determinism.ft runs 100 times, each under a different PYTHONHASHSEED; the 100 outputs (iteration order of a
     Map and two Sets of 257 entries, and their canonical text) must be identical (LANG-06-06).

  python3 -I conformance/tools/codec_check.py [--freeze] [--runs N]
  --freeze writes golden files that do not exist yet; it never replaces one."""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "conformance", "tools"))
CASES = os.path.join(ROOT, "conformance", "codec", "cases")
GOLDEN = os.path.join(ROOT, "conformance", "codec", "golden")
ROUND_TRIP = ("builtins", "derived")


def text_canonical(data: bytes) -> str | None:
    text = data.decode("utf-8")
    again = json.dumps(json.loads(text), separators=(",", ":"), ensure_ascii=False)
    return None if again == text else f"re-serialises as {again[:80]}"


def binary_wellformed(data: bytes) -> str | None:
    """Walk FORMAT.md's tags without types; the value must end exactly at the end of the input."""
    if data[:4] != b"FTB\x01":
        return "missing header"
    pos = [4]

    def byte():
        if pos[0] >= len(data): raise ValueError("truncated")
        pos[0] += 1
        return data[pos[0] - 1]

    def leb():
        n, shift, start = 0, 0, pos[0]
        while True:
            b = byte()
            n |= (b & 0x7F) << shift
            shift += 7
            if not b & 0x80:
                if b == 0 and pos[0] - start > 1: raise ValueError("non-minimal LEB128")
                return n

    def value():
        t = byte()
        if t in (0x00, 0x01, 0x02): return
        if t in (0x03, 0x0C, 0x0D, 0x0E): leb(); return
        if t == 0x04: pos[0] += 8; return
        if t == 0x0F: pos[0] += 4; return
        if t == 0x05:
            if byte() not in (0, 1): raise ValueError("decimal sign")
            if byte() > 28: raise ValueError("decimal scale")
            leb(); return
        if t in (0x06, 0x07):
            n = leb(); pos[0] += n
            if t == 0x06: data[pos[0] - n:pos[0]].decode("utf-8")
            return
        if t in (0x08, 0x0A, 0x10, 0x11):
            for _ in range(leb()): value()
            return
        if t == 0x09:
            for _ in range(leb()): value(); value()
            return
        if t == 0x0B:
            leb()
            for _ in range(leb()): value()
            return
        raise ValueError(f"unknown tag {t:#04x}")
    try:
        value()
    except (ValueError, UnicodeDecodeError) as e:
        return str(e)
    return None if pos[0] == len(data) else f"{len(data) - pos[0]} trailing bytes"


def run(case: str, env_extra: dict) -> subprocess.CompletedProcess:
    env = dict(os.environ)
    env.update(env_extra)
    return subprocess.run([sys.executable, "-I", os.path.join(ROOT, "interpreter", "run.py"), os.path.join(CASES, case + ".ft")],
                          capture_output=True, text=True, env=env, cwd=ROOT, timeout=300)


def main(argv):
    import names
    import parser
    from checker import check
    freeze = "--freeze" in argv
    runs = int(argv[argv.index("--runs") + 1]) if "--runs" in argv else 100
    rec = parser.load_grammar(os.path.join(ROOT, "reference", "grammar", "fortis.ebnf"))
    lib = names.Library(rec)
    problems, report = [], {"cases": {}, "determinism": {}}
    for f in sorted(os.listdir(CASES)):
        if f.endswith(".ft"):
            src = open(os.path.join(CASES, f), encoding="utf-8").read()
            diags = [r for r in check.check_source(lib, src, f"conformance/codec/cases/{f}") if r.get("code")]
            for r in diags:
                problems.append(f"{f}: checker {r['code']} line {r['line']}: {r['message'][:100]}")
    for case in ROUND_TRIP:
        root = tempfile.mkdtemp(prefix=f"codec-{case}-")
        p = run(case, {"FORTIS_DEV_ROOT": root})
        lines = [l for l in p.stdout.split("\n") if l.strip()]
        if p.returncode != 0:
            problems.append(f"{case}: exit {p.returncode}: {(p.stderr or p.stdout).strip().splitlines()[-1:]}")
        bad = [l for l in lines if not l.endswith(" true true")]
        problems += [f"{case}: round trip failed: {l}" for l in bad]
        gdir = os.path.join(GOLDEN, case)
        os.makedirs(gdir, exist_ok=True)
        written = sorted(x for x in os.listdir(root) if x.endswith((".json", ".bin")))
        golden = sorted(x for x in os.listdir(gdir) if x.endswith((".json", ".bin")))
        matched, frozen = 0, []
        for x in written:
            data = open(os.path.join(root, x), "rb").read()
            why = text_canonical(data) if x.endswith(".json") else binary_wellformed(data)
            if why:
                problems.append(f"{case}: {x} is not canonical per FORMAT.md: {why}")
            g = os.path.join(gdir, x)
            if not os.path.exists(g):
                if freeze:
                    open(g, "wb").write(data)
                    frozen.append(x)
                else:
                    problems.append(f"{case}: {x} has no golden file (run with --freeze to record it)")
                continue
            if open(g, "rb").read() != data:
                problems.append(f"{case}: {x} differs from its golden file")
            else:
                matched += 1
        for x in golden:
            if x not in written:
                problems.append(f"{case}: golden {x} was not written by the case (stale)")
        values = len({x.rsplit(".", 1)[0] for x in written})
        report["cases"][case] = {"values": values, "round_trips": len(lines) - len(bad), "files": len(written),
                                 "matched_golden": matched, "frozen_now": frozen}
        print(f"{case:<9} {values:2} values, {len(lines) - len(bad):2} round trips in both formats, "
              f"{matched}/{len(written)} files equal their golden files" + (f", {len(frozen)} frozen now" if frozen else ""))
    digests = {}
    for i in range(runs):
        p = run("determinism", {"PYTHONHASHSEED": str(i)})
        if p.returncode != 0:
            problems.append(f"determinism run {i}: exit {p.returncode}")
            continue
        digests.setdefault(hashlib.sha256(p.stdout.encode()).hexdigest(), []).append(i)
    # control: the seeds do perturb hashing, so an order taken from a hash table would differ across these runs
    control = set()
    for i in range(min(runs, 10)):
        env = dict(os.environ, PYTHONHASHSEED=str(i))
        control.add(subprocess.run([sys.executable, "-I", "-c", "print(list({f'w{k}' for k in range(300)}))"],
                                   capture_output=True, text=True, env=env).stdout)
    if runs > 1 and len(control) < 2:
        problems.append("determinism control: the hash seeds did not perturb a Python set, so the test has no power")
    report["determinism"] = {"runs": runs, "distinct_outputs": len(digests), "digests": {k: len(v) for k, v in digests.items()},
                             "control_distinct_orders_of_a_hash_set": len(control)}
    if len(digests) != 1:
        problems.append(f"determinism: {len(digests)} distinct outputs over {runs} runs")
    print(f"determinism: {runs} runs under {runs} hash seeds, {len(digests)} distinct output" + ("" if len(digests) == 1 else "s")
          + f" (control: a Python hash set of the same keys took {len(control)} orders under {min(runs, 10)} of the seeds)")
    for pr in problems:
        print("FAIL", pr)
    report["problems"] = problems
    json.dump(report, open(os.path.join(ROOT, "conformance", "codec", "results.json"), "w"), indent=1)
    print(f"\n{len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
