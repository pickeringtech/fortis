"""Fortis performance against hand-written Rust (PROG-NFR-09, DEL-40).

Each benchmark is a Fortis program (benchmarks/fortis/<name>.ft) and a Rust program performing the same
operations in the same order (benchmarks/rust/<name>.rs), reviewed independently for equal work
(docs/reviews/del40-kernel-review-*.md). The Fortis program is checked, lowered by the production lowering
(`fortisc native`: `#![forbid(unsafe_code)]`, overflow checks on, `panic = "abort"`, codegen-units 1, release) and
built; the Rust program is built with the same code-generation settings
(`rustc -C opt-level=3 -C codegen-units=1 -C overflow-checks=yes -C panic=abort --edition=2021`, Cargo's release
level).

Method: each program runs once untimed; then N rounds, each running both programs in an order drawn from a seeded
generator. Every run must exit 0 and print the benchmark's expected output, and every benchmark must be present.
Reported per benchmark: the median, minimum and maximum of each side, the ratio of medians, and a 95% bootstrap
interval of that ratio that resamples whole rounds (both programs of a round together, 2,000 resamples, seed 0).
The SHOULD budget is a ratio of at most 1.05: met when the ratio and the interval's upper end are within it,
met but uncertain when only the ratio is. A baseline pair (benchmarks/baseline: start, set up, print one line)
shows the fixed cost each side's timing includes.

  python3 -I benchmarks/run.py [--runs N] [--vm] [--json]"""
from __future__ import annotations

import json
import os
import random
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
# the corpus reads captured values, so it runs under the permit-all telemetry plan (Language Reference 15; D-25)
os.environ.setdefault("FORTIS_TELEMETRY", "permit-all")
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
OUT = os.path.join(ROOT, "build", "bench")
RUSTC = ["rustc", "-C", "opt-level=3", "-C", "codegen-units=1", "-C", "overflow-checks=yes", "-C", "panic=abort", "--edition=2021"]
# each benchmark's output (both programs must print exactly this)
EXPECTED = {
    "baseline": "0",
    "fib": "14930352",
    "lists": "5000000 149999970000000",
    "loops": "5860907551599735424",
    "maps": "1000 10000",
    "objects": "-4956933368772027770",
    "sort": "4000000 0 499911 999999",
    "strings": "52888890",
}
KERNELS = ["fib", "lists", "loops", "maps", "objects", "sort", "strings"]


def build(name, fortis_src, rust_src):
    """Both executables, freshly built; a failed build is an error, never a stale binary."""
    crate = "fx_" + "".join(c.lower() if c.isalnum() else "_" for c in name)
    exe = os.path.join(ROOT, "build", "native-prod", "target", "release", crate)
    if os.path.exists(exe):
        os.remove(exe)
    r = subprocess.run([FORTISC, "native", fortis_src], capture_output=True, text=True, cwd=ROOT, timeout=1200).stdout
    if not os.path.exists(exe) or "build-error" in r or "refused" in r:
        raise SystemExit(f"{name}: the native build failed: {r[-400:]}")
    rust_exe = os.path.join(OUT, name + "_rust")
    if os.path.exists(rust_exe):
        os.remove(rust_exe)
    subprocess.run(RUSTC + ["-o", rust_exe, rust_src], check=True)
    return exe, rust_exe


def run_once(exe, want):
    t0 = time.perf_counter()
    r = subprocess.run([exe], capture_output=True, text=True, timeout=600)
    t = time.perf_counter() - t0
    if r.returncode != 0:
        raise SystemExit(f"{exe} exited {r.returncode}: {r.stderr[-300:]}")
    if want is not None and r.stdout != want + "\n":
        raise SystemExit(f"{exe} printed {r.stdout!r}, expected {want!r}")
    return t, r.stdout


def measure(name, fexe, rexe, runs, rnd):
    want = EXPECTED[name]
    _, fo = run_once(fexe, want)
    _, ro = run_once(rexe, want)
    if fo != ro:
        raise SystemExit(f"{name}: outputs differ: Fortis {fo!r}, Rust {ro!r}")
    want = fo.rstrip("\n")
    rounds = []
    for _ in range(runs):
        order = [("f", fexe), ("r", rexe)]
        rnd.shuffle(order)
        times = {}
        for side, exe in order:
            times[side] = run_once(exe, want)[0]
        rounds.append((times["f"], times["r"]))
    return rounds, want


def ratio_ci(rounds, reps=2000, seed=0):
    """A 95% interval of median(Fortis) / median(Rust), resampling whole rounds."""
    rnd = random.Random(seed)
    rs = []
    for _ in range(reps):
        sample = rnd.choices(rounds, k=len(rounds))
        rs.append(statistics.median(f for f, _ in sample) / statistics.median(r for _, r in sample))
    rs.sort()
    return rs[int(0.025 * reps)], rs[int(0.975 * reps) - 1]


def main(argv):
    runs = int(argv[argv.index("--runs") + 1]) if "--runs" in argv else 21
    os.makedirs(OUT, exist_ok=True)
    rnd = random.Random(20261024)
    rows = []
    pairs = [("baseline", os.path.join("benchmarks", "baseline", "baseline.ft"), os.path.join(HERE, "baseline", "baseline.rs"))]
    pairs += [(n, os.path.join("benchmarks", "fortis", n + ".ft"), os.path.join(HERE, "rust", n + ".rs")) for n in KERNELS]
    missing = [n for n in KERNELS if not os.path.exists(os.path.join(HERE, "fortis", n + ".ft")) or not os.path.exists(os.path.join(HERE, "rust", n + ".rs"))]
    if missing:
        raise SystemExit(f"missing benchmarks: {missing}")
    for name, fsrc, rsrc in pairs:
        fexe, rexe = build(name, fsrc, rsrc)
        rounds, out = measure(name, fexe, rexe, runs, rnd)
        fs, rs_ = [f for f, _ in rounds], [r for _, r in rounds]
        tf, tr = statistics.median(fs), statistics.median(rs_)
        lo, hi = ratio_ci(rounds)
        ratio = tf / tr
        verdict = "met" if ratio <= 1.05 and hi <= 1.05 else ("met, interval straddles 1.05" if ratio <= 1.05 else "not met")
        row = {"benchmark": name, "output": out, "fortis_median_s": round(tf, 5), "rust_median_s": round(tr, 5),
               "fortis_min_max_s": [round(min(fs), 5), round(max(fs), 5)], "rust_min_max_s": [round(min(rs_), 5), round(max(rs_), 5)],
               "ratio": round(ratio, 4), "ratio_ci95": [round(lo, 4), round(hi, 4)], "verdict": verdict if name != "baseline" else "fixed cost",
               "rounds": [[round(f, 6), round(r, 6)] for f, r in rounds]}
        if "--vm" in argv and name != "baseline":
            t0 = time.perf_counter()
            vm = subprocess.run([FORTISC, "vm", fsrc], capture_output=True, text=True, cwd=ROOT, timeout=3600).stdout
            row["vm_s"] = round(time.perf_counter() - t0, 3)
            row["vm_output_equal"] = f"observed: {out}" in vm
        rows.append(row)
        print(f"{name:<9} Fortis {tf*1000:8.2f} ms [{min(fs)*1000:.2f}–{max(fs)*1000:.2f}]   Rust {tr*1000:8.2f} ms [{min(rs_)*1000:.2f}–{max(rs_)*1000:.2f}]   "
              f"ratio {ratio:6.3f} (95% CI {lo:.3f}–{hi:.3f})   {row['verdict']}")
    kernels = [r for r in rows if r["benchmark"] != "baseline"]
    geo = statistics.geometric_mean([r["ratio"] for r in kernels])
    print(f"\n{sum(r['verdict'].startswith('met') for r in kernels)} of {len(kernels)} kernels within 5%; geometric mean ratio {geo:.3f} (descriptive only); {runs} rounds each")
    if "--json" in argv:
        json.dump({"rule": "PROG-NFR-09", "rustc": RUSTC, "runs": runs, "rows": rows}, open(os.path.join(OUT, "results.json"), "w"), indent=1)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
