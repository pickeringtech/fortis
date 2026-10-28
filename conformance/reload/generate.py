#!/usr/bin/env python3
"""Write a version of the SP-07 workload (spikes/SP-07/design.md 11).

A version is a number `v` and a set of edits from edits.json. The edits are textual replacements applied to
template.ft in catalogue order. Then each `{{X}}` placeholder becomes the version's coefficient X, a literal in the
body of the function that uses it.

    python3 conformance/reload/generate.py V [EDIT ...] > out.ft
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def coefficients(v):
    """Each coefficient as a function of the version, so the oracle knows every version's formulas."""
    return {"V": v, "A": v + 1, "B": v, "C": 3 * v, "F": v, "L": 100 * v, "T": v + 3, "R": 10 * v, "S": v + 1, "W": 2 * v + 1}


def catalogue():
    with open(os.path.join(HERE, "edits.json"), encoding="utf-8") as f:
        return json.load(f)


def text(v, edits):
    cat = catalogue()
    names = [e["name"] for e in cat["edits"]]
    for e in edits:
        if e not in names:
            raise SystemExit(f"unknown edit {e}")
    with open(os.path.join(HERE, "template.ft"), encoding="utf-8") as f:
        s = f.read()
    for e in cat["edits"]:
        if e["name"] not in edits:
            continue
        for need in e.get("requires", []):
            if need not in edits:
                raise SystemExit(f"edit {e['name']} requires {need}")
        for old, new in e["replace"]:
            if s.count(old) != 1:
                raise SystemExit(f"edit {e['name']}: {old!r} occurs {s.count(old)} times")
            s = s.replace(old, new)
    co = coefficients(v)
    return re.sub(r"\{\{(\w+)\}\}", lambda m: str(co[m.group(1)]), s)


def name(v, edits):
    """A version's file name: its number and edits, or a digest of the edits when they would make it too long."""
    full = "v%d%s.ft" % (v, "".join("+" + e for e in sorted(edits)))
    if len(full) <= 120:
        return full
    import hashlib
    return "v%d+%d-edits-%s.ft" % (v, len(edits), hashlib.sha256(",".join(sorted(edits)).encode()).hexdigest()[:12])


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    sys.stdout.write(text(int(sys.argv[1]), sys.argv[2:]))
