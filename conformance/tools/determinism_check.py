"""Native builds are deterministic: a sample of programs, each built twice from scratch, gives the same bytes.

    python3 -I conformance/tools/determinism_check.py [--json]

For every program of SAMPLE, `fortisc build` writes an executable; its generated crate (build/native-build/fb_*)
is then removed, so that the second build compiles it again rather than reusing it, and the two executables must
be byte-identical. (build_check.py, run without --no-clones, does the same for the compiler itself and one
application, from two clean clones.) The release workflow runs both.
"""
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
# the examples with a main, a package, and programs that exercise the runtime's larger parts
SAMPLE = ["examples/01_hello.ft", "examples/02_outcomes.ft", "examples/03_ownership.ft", "examples/04_collections.ft",
          "examples/05_types.ft", "examples/06_numeric.ft", "examples/apps/orders", "acceptance/a2",
          "conformance/programs/exec_scope_defer.ft", "conformance/programs/exec_interaction_pure_closures.ft"]


def sha(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def crate_dir(program):
    stem = os.path.basename(program.rstrip("/"))
    stem = stem[:-3] if stem.endswith(".ft") else stem
    return os.path.join(ROOT, "build", "native-build", "fb_" + re.sub(r"[^a-z0-9]", "_", stem.lower()))


def build(program, out):
    r = subprocess.run([FORTISC, "build", "--format", "json", "--out", out, program], cwd=ROOT, capture_output=True, text=True, timeout=1800)
    return r.returncode == 0 and os.path.exists(out), (r.stdout + r.stderr)[-400:]


def main(argv):
    work = tempfile.mkdtemp(prefix="fortis-determinism-")
    rows, problems = [], 0
    try:
        for program in SAMPLE:
            if not os.path.exists(os.path.join(ROOT, program)):
                rows.append({"program": program, "ok": False, "detail": "missing"})
                problems += 1
                print(f"FAIL {program}: missing")
                continue
            digests, detail = [], ""
            for run in ("first", "second"):
                shutil.rmtree(crate_dir(program), ignore_errors=True)       # compiled again, not reused
                out = os.path.join(work, run, os.path.basename(program.rstrip("/")))
                os.makedirs(os.path.dirname(out), exist_ok=True)
                ok, detail = build(program, out)
                digests.append(sha(out) if ok else None)
            same = digests[0] is not None and digests[0] == digests[1]
            problems += not same
            rows.append({"program": program, "ok": same, "sha256": digests})
            print(f"{'PASS' if same else 'FAIL'} {program}: " + (f"two builds, identical ({digests[0][:16]})" if same else f"{digests} {detail}"))
    finally:
        shutil.rmtree(work, ignore_errors=True)
    if "--json" in argv:
        os.makedirs(os.path.join(ROOT, "build"), exist_ok=True)
        json.dump({"programs": rows}, open(os.path.join(ROOT, "build", "determinism.json"), "w"), indent=1)
    print(f"\n{len(rows)} programs, each built twice: {len(rows) - problems} identical")
    print("determinism check: " + ("OK" if not problems else "FAILED"))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
