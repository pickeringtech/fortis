#!/usr/bin/env python3
"""Fetch the toolchain this workflow run's build job publishes (its `tools` artifact) into the checkout, for a gate
shard that started before the build ended (`gate.py --toolchain`, ci.yml).

    python3 -I .github/scripts/fetch_tools.py [--deadline SECONDS]

It waits for the artifact, checking every 10 seconds, retries a failed download up to five times with a growing
wait, and stops at the deadline (25 minutes by default) or as soon as the build job ends without one, and says which. Every `gh` call has its own deadline. It needs GH_TOKEN (a token
that may read the run's artifacts), GITHUB_REPOSITORY and GITHUB_RUN_ID, which a workflow job has.
"""
import os
import shutil
import subprocess
import sys
import tarfile
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))


def gh(args, timeout=60):
    try:
        r = subprocess.run(["gh"] + args, capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout.strip() or r.stderr.strip())
    except subprocess.TimeoutExpired:
        return 124, f"gh {' '.join(args[:2])} passed its {timeout} s deadline"


def main(argv):
    deadline = int(argv[argv.index("--deadline") + 1]) if "--deadline" in argv else 1500
    repo, run = os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_RUN_ID"]
    start, polls, build = time.time(), 0, "not seen yet"
    while True:
        polls += 1
        code, out = gh(["api", f"repos/{repo}/actions/runs/{run}/artifacts?name=tools", "--jq", ".artifacts | length"])
        if code == 0 and out not in ("", "0"):
            break
        # the build job's state, every third check: a build that failed publishes nothing
        if polls % 3 == 1:
            code, out = gh(["api", f"repos/{repo}/actions/runs/{run}/jobs?per_page=100", "--jq", '.jobs[] | select(.name == "build") | "\\(.status) \\(.conclusion)"'])
            build = out.splitlines()[-1] if code == 0 and out else f"unknown ({out[:200]})"
            if build.startswith("completed") and not build.endswith("success"):
                print(f"fetch_tools: the build job ended without the toolchain ({build}), after {time.time() - start:.0f} s")
                return 1
        if time.time() - start > deadline:
            print(f"fetch_tools: TIMEOUT: no `tools` artifact after {deadline} s and {polls} checks; the build job: {build}")
            return 1
        time.sleep(10)
    dest = os.path.join(ROOT, "build", "tools-download")
    # GitHub's artifact storage throttles now and then ("HTTP 503: Egress is over the account limit", 30 Oct 2026,
    # about 15 minutes): wait and try again, five times at most and within the deadline, before failing the shard
    for attempt in range(1, 6):
        shutil.rmtree(dest, ignore_errors=True)
        code, out = gh(["run", "download", run, "--name", "tools", "--dir", dest], timeout=600)
        if code == 0:
            break
        wait = 60 * attempt
        if attempt == 5 or time.time() - start + wait > deadline:
            print(f"fetch_tools: downloading the `tools` artifact failed (attempt {attempt}): {out[:300]}")
            return 1
        print(f"fetch_tools: downloading the `tools` artifact failed (attempt {attempt}), retrying in {wait} s: {out[:200]}")
        time.sleep(wait)
    with tarfile.open(os.path.join(dest, "tools.tar")) as t:
        t.extractall(ROOT, filter="data")
    shutil.rmtree(dest, ignore_errors=True)
    print(f"fetch_tools: the toolchain arrived after {time.time() - start:.0f} s ({polls} checks)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
