"""The maintainers' planning files, which some checks also read when they are present.

The maintainers' checkout of Fortis holds the delivery plan, the decision register, the Delivery Programme and the
spike records beside the language. The public repository does not. A check that compares against one of those
files does that part only when the file is present. When it is absent, the check says so in one line on standard
error, naming what it skipped and why, and carries on with everything else: an absent planning file never fails a
check, and never makes one pass quietly.

    path = planning.optional(planning.DEFECTS, "docset", "matching failures against the open defects")
    if path: ...
"""
from __future__ import annotations

import os
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))

# the planning files the checks read, relative to the repository root
PROGRAMME = "docs/Fortis_Delivery_Programme.md"     # the Delivery Programme (PROG- rules, sections)
REGISTER = "docs/Fortis_Specification_Review.md"     # the decision register (X- rows)
DEFECTS = "docs/delivery/defects.json"               # the defect register (D- entries)
SUBSETS = "docs/delivery/subsets.json"               # the G1 subset of the language
OWNERSHIP = "docs/delivery/ownership.json"           # which delivery chunk owns each rule and code
PRIVILEGES = "docs/delivery/privileges.json"         # the privilege declarations of the tools
RELEASES = "docs/delivery/releases.md"               # the release ladder
SPIKES = "spikes"                                    # the spike records and their registry
SPIKE_REGISTRY = "spikes/registry.json"
PLAN_TOOLS = "docs/delivery/tools"                   # the plan's own checks

_said = set()


def present(rel: str) -> bool:
    return os.path.exists(os.path.join(ROOT, rel))


def skipped(tool: str, what: str, rel: str) -> None:
    """Say, once per process, that `tool` skipped `what` because `rel` is not in this checkout."""
    key = (tool, what, rel)
    if key in _said:
        return
    _said.add(key)
    print(f"{tool}: skipped {what}: {rel} is a maintainers' planning file and is not in this checkout", file=sys.stderr, flush=True)


def optional(rel: str, tool: str, what: str) -> str | None:
    """The absolute path of planning file `rel`, or None after saying what `tool` skips without it."""
    full = os.path.join(ROOT, rel)
    if os.path.exists(full):
        return full
    skipped(tool, what, rel)
    return None


def mirrored(rel: str, copy: str, tool: str, what: str) -> str | None:
    """A planning file that public checks need, with a public copy beside the checks. Where `rel` is present, its
    bytes are also written to `copy` (when they differ), so the copy follows it and is committed with the results;
    the path of `rel` is returned. Where it is absent, the copy's path is returned, after one line saying so. None
    when neither exists, after saying what `tool` skips."""
    full, copy_full = os.path.join(ROOT, rel), os.path.join(ROOT, copy)
    if os.path.exists(full):
        data = open(full, "rb").read()
        if not os.path.exists(copy_full) or open(copy_full, "rb").read() != data:
            with open(copy_full, "wb") as out:
                out.write(data)
        return full
    if os.path.exists(copy_full):
        key = (tool, "copy", rel)
        if key not in _said:
            _said.add(key)
            print(f"{tool}: {rel} is a maintainers' planning file and is not in this checkout; using its public copy, {copy}",
                  file=sys.stderr, flush=True)
        return copy_full
    skipped(tool, what + f" (and its public copy, {copy}, is missing)", rel)
    return None
