#!/usr/bin/env python3
"""The SP-12 invalidation oracle (SP-12's design, section 8): for each edit, the passes that must run and the passes
that may, from the workload's manifest and the edit's description alone. It reads neither the daemon nor the checker.

A pass is `(file, unit, pass)`. A *may* entry `(file, None, None)` stands for every pass of every unit of a module,
and `(file, unit, None)` for both passes of one unit. The *may* set is the design's granularity: within a module, a
shifted skeleton reruns every unit, because a pass reads the module's interface, whose skeleton carries positions;
across modules, only the units whose foreign closure names the edited declaration rerun.
"""
import json
import os


def num(path):
    return int(os.path.basename(path)[1:6])


class Oracle:
    def __init__(self, pkg_dir):
        self.man = json.load(open(os.path.join(pkg_dir, "manifest.json"), encoding="utf-8"))
        key_file = {m["key"]: m["file"] for m in self.man["modules"]}
        self.first_of, self.alias_of = {}, {}
        for m in self.man["modules"]:
            imps = [key_file[k] for k in m["imports"]]
            if imps:
                self.first_of.setdefault(imps[0], []).append(m["file"])
            if len(imps) > 1:
                self.alias_of.setdefault(imps[1], []).append(m["file"])
        self.hubs = self.man["hubs"]
        # (module, group) -> the modules whose group calls that module's clampTo through a member import (the
        # generator's rule: group i uses the import i mod 3; the second is the alias import)
        self.member_users = {}
        for m in self.man["modules"]:
            imps = [key_file[k] for k in m["imports"]]
            for i in range(10):
                if imps and i % len(imps) != 1:
                    self.member_users.setdefault((imps[i % len(imps)], i), []).append(m["file"])

    def sets(self, e):
        """(must, may) for edit description `e` (edits.Edit.describe())."""
        kind, role, f, unit = e["kind"], e["role"], e["file"], e.get("unit")
        whole = {(f, None, None)}
        if kind == "body-literal":
            return {(f, unit, 2)}, {(f, unit, None)}
        if kind in ("body-line", "body-error", "ownership", "obligation"):
            return {(f, unit, 2)}, whole
        if kind == "effect":
            g = unit.split("step", 1)[1]
            return {(f, unit, 2), (f, f"decl fast{g}", 2)}, whole
        if kind == "comment":
            return set(), whole
        if kind in ("sig-compat", "sig-break") or (kind == "probe" and role in ("open", "restore")):
            callers = self.hubs[e["hub"]]["callers"]
            must = {(cf, f"decl {c}", 2) for cf, c in callers}
            return must, whole | {(cf, f"decl {c}", None) for cf, c in callers}
        if kind == "probe":
            # a line above the hub: its callers' records carry no position of it, and they do not rerun (A1)
            return set(), whole
        if kind == "decl":
            # the module's exports change: a unit that names the module through an alias reads them (E-NAM-004)
            al = self.alias_of.get(f, [])
            return ({(f, unit, 2)} if role == "open" else set()), whole | {(u, None, None) for u in al}
        if kind in ("rename", "syntax"):
            # the resolver answers alone; its restoring edit finds every memo as it was
            return set(), set()
        users = self.first_of.get(f, [])
        if kind == "iface":
            # a class implementing the interface names it in its head, so a module whose `link` names that class reads
            # the interface too: the modules whose first import is a user (heads reach through heads, design 3.2)
            second = {(v, f"decl link{num(v)}u0", None) for u in users for v in self.first_of.get(u, [])}
            return {(u, f"decl Item{num(u)}u0", 2) for u in users}, whole | {(u, None, None) for u in users} | second
        if kind == "contract":
            return {(u, f"decl lean{num(u)}u0", 2) for u in users}, whole | {(u, None, None) for u in users}
        if kind == "aspect":
            # the hook's records carry its positions: its users rerun when it breaks, moves or is restored
            return {(u, f"decl link{num(u)}u0", 2) for u in users}, whole | {(u, None, None) for u in users}
        if kind == "alias":
            i = e["group"]
            al = self.alias_of.get(f, [])
            return {(u, f"decl {fn}{num(u)}u{i}", 2) for u in al for fn in ("link", "shown")}, whole | {(u, None, None) for u in al}
        if kind == "member":
            i = e["group"]
            us = self.member_users.get((f, i), [])
            return {(u, f"decl {fn}{num(u)}u{i}", 2) for u in us for fn in ("link", "shown")}, whole | {(u, None, None) for u in us}
        raise SystemExit(f"the oracle has no rule for {kind}")

    @staticmethod
    def check(must, may, ran):
        """Whether the passes that ran include `must` and lie within `may`; and what was missing or extra."""
        ran = {(a, b, c) for a, b, c in ran}
        missing = sorted(must - ran)
        extra = sorted(p for p in ran if not ((p[0], None, None) in may or (p[0], p[1], None) in may or p in may))
        return not missing and not extra, missing[:5], extra[:5]
