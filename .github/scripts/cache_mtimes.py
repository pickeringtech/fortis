#!/usr/bin/env python3
"""Make CI's restored build cache usable by Cargo without ever making it trust stale output.

Cargo decides that a local crate is fresh by comparing file times: a source newer than the crate's last build, or
a dependency's output newer than the crate's own, makes it rebuild. A fresh checkout gives every file the time of
the checkout, and a restored cache does not reliably keep its outputs' times in order (CI rebuilt every crate from
a cache saved by a job that had itself only restored it). So both sides are set here, after the checkout and the
cache restore:

- every file of the cached build directories (DIRS) gets one time, OUT, the moment of the restore, so no restored
  output is newer than another;
- a tracked file whose bytes are exactly those `save` recorded gets its recorded time, older than OUT, so Cargo
  reuses what was built from it;
- a tracked file whose bytes differ, and one the record does not name, gets OUT plus one second, newer than every
  restored output, so Cargo rebuilds whatever depends on it.

Nothing here can make Cargo reuse an output built from different source: it can only change how long a build takes.

    python3 -I .github/scripts/cache_mtimes.py save       at the end of a job, before the cache is saved
    python3 -I .github/scripts/cache_mtimes.py restore    after the checkout and the cache restore
"""
import hashlib
import json
import os
import subprocess
import sys
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
MANIFEST = os.path.join(ROOT, "build", "ci-cache", "mtimes.json")
# the build directories CI caches (ci.yml): Cargo's target directories and the native backend's
DIRS = ["compiler/target", "build/native-build", "build/native-prod", "build/native-runtime"]
SECOND = 1_000_000_000


def tracked():
    out = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, timeout=120, check=True).stdout
    return [p for p in out.decode("utf-8").split("\0") if p]


def sha(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def regular(rel):
    full = os.path.join(ROOT, rel)
    return full if os.path.isfile(full) and not os.path.islink(full) else None


def save():
    record = {}
    for rel in tracked():
        full = regular(rel)
        if full:
            record[rel] = [sha(full), os.stat(full).st_mtime_ns]
    os.makedirs(os.path.dirname(MANIFEST), exist_ok=True)
    with open(MANIFEST, "w", encoding="utf-8") as out:
        json.dump(record, out)
    print(f"cache-mtimes: recorded {len(record)} files in {os.path.relpath(MANIFEST, ROOT)}")
    return 0


def restore():
    if not os.path.exists(MANIFEST):
        print("cache-mtimes: no record (no cache restored); every file keeps the checkout's time")
        return 0
    record = json.load(open(MANIFEST, encoding="utf-8"))
    out = time.time_ns()
    outputs = 0
    for d in DIRS:
        for base, _, files in os.walk(os.path.join(ROOT, d)):
            for f in files:
                p = os.path.join(base, f)
                if not os.path.islink(p):
                    os.utime(p, ns=(out, out))
                    outputs += 1
    same = changed = new = 0
    for rel in tracked():
        full = regular(rel)
        if not full:
            continue
        entry = record.get(rel)
        if entry is not None and sha(full) == entry[0]:
            t = min(entry[1], out - SECOND)
            same += 1
        else:
            t = out + SECOND
            changed += entry is not None
            new += entry is None
        os.utime(full, ns=(t, t))
    print(f"cache-mtimes: {outputs} cached build files set to one time; {same} sources unchanged since the cached "
          f"build get their recorded times; {changed} changed and {new} new get a later time, so Cargo rebuilds what "
          f"depends on them")
    return 0


if __name__ == "__main__":
    if sys.argv[1:] == ["save"]:
        sys.exit(save())
    if sys.argv[1:] == ["restore"]:
        sys.exit(restore())
    sys.exit(__doc__)
