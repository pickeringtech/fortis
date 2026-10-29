#!/usr/bin/env python3
"""The SP-12 edit script (SP-12's design, section 5): a deterministic editing session over the application workload.

`Script(pkg_dir, seed, plan)` reads the package's texts and manifest, and yields edits one at a time. An edit is the
whole new text of one module (the daemon's full-document sync) with its description: its kind, its role (`open`,
`restore`, a `step` of a probe, or the `inverse` of a soak edit), the module and unit it touches, and what the oracle
needs to know about it. Each edit applies to the current texts: the session is cumulative.

Every kind is a replacement at a place the generator (generate.py) wrote, found by an anchor that names the
declaration. A kind that leaves the package in error (`body-error`, `sig-break` and the probes) opens a pair whose
restoring edit comes 1 to 15 edits later; `syntax` and `rename`, which leave only the resolver's records, are
restored by the very next edit. A site is busy while a pair on it is open, and no other kind edits it.

    python3 -I benchmarks/incremental/edits.py build/incremental/app-50k --seed 12 --count 1050   # prints the script
"""
import json
import os
import random
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import generate  # noqa: E402

# the latency run's openings per kind (design 5): 1,050 edits in all, with the restores and probe steps
LATENCY = {
    "body-literal": 330, "body-line": 160, "body-error": 40, "comment": 40,
    "sig-compat": 60, "sig-break": 60, "probe": 5,
    "decl": 20, "rename": 10, "syntax": 10,
    "effect": 10, "ownership": 10, "obligation": 10,
    "iface": 10, "contract": 10, "aspect": 10, "alias": 10, "member": 10,
}

# the apparatus check's reduced run (design 10): every kind, about 210 edits
REDUCED = {
    "body-literal": 50, "body-line": 30, "body-error": 8, "comment": 8,
    "sig-compat": 10, "sig-break": 10, "probe": 2,
    "decl": 4, "rename": 3, "syntax": 3,
    "effect": 3, "ownership": 3, "obligation": 3,
    "iface": 3, "contract": 3, "aspect": 2, "alias": 3, "member": 3,
}

# kinds whose opening leaves an error that a later edit restores (and the edits a probe makes in order)
PAIRED = {"body-error", "sig-break", "decl", "rename", "syntax", "effect", "ownership", "obligation", "iface", "contract", "alias", "member"}
CHAINS = {"probe": ["break", "shift", "unshift", "restore"], "aspect": ["break", "shift", "unshift", "restore"]}
NEXT = {"rename", "syntax"}


class Edit:
    def __init__(self, kind, role, file, text, unit=None, **meta):
        self.kind, self.role, self.file, self.text, self.unit, self.meta = kind, role, file, text, unit, meta

    def describe(self):
        return {"kind": self.kind, "role": self.role, "file": self.file, "unit": self.unit, **self.meta}


def num(path):
    return int(os.path.basename(path)[1:6])


class Script:
    def __init__(self, pkg_dir, seed, plan=None, cycle=0):
        self.dir = pkg_dir
        self.man = json.load(open(os.path.join(pkg_dir, "manifest.json"), encoding="utf-8"))
        self.files = [m["file"] for m in self.man["modules"]]
        self.key_file = {m["key"]: m["file"] for m in self.man["modules"]}
        self.texts = {f: open(os.path.join(pkg_dir, f), encoding="utf-8").read() for f in self.files}
        self.original = dict(self.texts)
        self.rng = random.Random(seed)
        self.plan = dict(plan or LATENCY)
        self.cycle = cycle
        self.busy = set()
        self.counter = 0
        # module -> the modules importing it as their first (member, with its interface, contract and aspect) and
        # second (alias) import
        self.first_of, self.alias_of = {}, {}
        for m in self.man["modules"]:
            imps = [self.key_file[k] for k in m["imports"]]
            if imps:
                self.first_of.setdefault(imps[0], []).append(m["file"])
            if len(imps) > 1:
                self.alias_of.setdefault(imps[1], []).append(m["file"])
        self.importers = {}
        for m in self.man["modules"]:
            for k in m["imports"]:
                self.importers.setdefault(self.key_file[k], []).append(m["file"])
        self.hubs = self.man["hubs"]
        self.hub_param = {h: "Int<0..1000000>" for h in self.hubs}
        # (module, group) -> the modules whose group calls that module's clampTo through a member import
        self.member_users = {}
        for m in self.man["modules"]:
            imps = [self.key_file[k] for k in m["imports"]]
            for i in range(10):
                if imps and i % len(imps) != 1:
                    self.member_users.setdefault((imps[i % len(imps)], i), []).append(m["file"])

    # ---- text places ------------------------------------------------------------------------------------------
    def place(self, f, anchor, pattern):
        """The span of `pattern`'s first match after `anchor` in module `f`, or None."""
        t = self.texts[f]
        a = t.find(anchor)
        if a < 0:
            return None
        m = re.compile(pattern).search(t, a)
        return m

    def put(self, f, start, end, new):
        t = self.texts[f]
        self.texts[f] = t[:start] + new + t[end:]
        return self.texts[f]

    def group(self, layer_min=0, needs=None):
        """A module and group, uniformly; `needs` filters modules."""
        files = [f for f, m in zip(self.files, self.man["modules"]) if m["layer"] >= layer_min and (needs is None or needs(f))]
        f = self.rng.choice(files)
        return f, num(f), self.rng.randrange(10)

    # ---- kinds ------------------------------------------------------------------------------------------------
    def body_literal(self):
        for _ in range(50):
            f, m, i = self.group()
            g = f"{m}u{i}"
            site = self.rng.choice([(f"try func priceOf{g}(", r"Int\.wrappingAdd\(found, (\d+)\)", f"decl priceOf{g}"),
                                    (f"func summary{g}(", r"while i < (\d+) ", f"decl summary{g}"),
                                    (f"recovers func safePrice{g}(", r"recover (\d+)\n", f"decl safePrice{g}")])
            if (f, site[2]) in self.busy:
                continue
            mt = self.place(f, site[0], site[1])
            if not mt:
                continue
            old = int(mt.group(1))
            new = self.rng.choice([v for v in range(0, 1000) if v != old][:200])
            self.put(f, mt.start(1), mt.end(1), str(new))
            return Edit("body-literal", "open", f, self.texts[f], site[2], module=m, group=i)
        return None

    def body_line(self):
        for _ in range(50):
            f, m, i = self.group()
            g = f"{m}u{i}"
            fn = self.rng.choice([f"func summary{g}(", f"func stock{g}(", f"try func find{g}("])
            unit = "decl " + fn.split("func ")[1].rstrip("(")
            if (f, unit) in self.busy:
                continue
            mt = self.place(f, fn, r"\{\n(  discard \d+\n)?")
            if not mt:
                continue
            if mt.group(1):
                self.put(f, mt.start(1), mt.end(1), "")
            else:
                self.counter += 1
                self.put(f, mt.end(), mt.end(), f"  discard {self.counter}\n")
            return Edit("body-line", "open", f, self.texts[f], unit, module=m, group=i)
        return None

    def comment(self):
        for _ in range(50):
            f, m, i = self.group()
            g = f"{m}u{i}"
            anchor = f"export enum Status{g} {{"
            t = self.texts[f]
            a = t.find(anchor)
            if a < 0:
                continue
            prev = t.rfind("\n", 0, a - 1)
            line = t[prev + 1:a]
            if line.startswith("// note"):
                self.put(f, prev + 1, a, "")
            else:
                self.counter += 1
                self.put(f, a, a, f"// note {self.counter}\n")
            return Edit("comment", "open", f, self.texts[f], None, module=m, group=i)
        return None

    def sig_compat(self):
        hubs = [h for h in self.hubs if ("hub", h) not in self.busy]
        if not hubs:
            return None
        h = self.rng.choice(hubs)
        f = self.hubs[h]["home"]
        old = self.hub_param[h]
        new = "Int<0..2000000>" if old == "Int<0..1000000>" else "Int<0..1000000>"
        self.texts[f] = self.texts[f].replace(f"export pure func {h}(value: {old})", f"export pure func {h}(value: {new})", 1)
        self.hub_param[h] = new
        return Edit("sig-compat", "open", f, self.texts[f], f"decl {h}", hub=h)

    def hub_break(self, kind):
        hubs = [h for h in self.hubs if ("hub", h) not in self.busy]
        if not hubs:
            return None, None
        h = self.rng.choice(hubs)
        f = self.hubs[h]["home"]
        old = self.hub_param[h]
        self.texts[f] = self.texts[f].replace(f"export pure func {h}(value: {old})", f"export pure func {h}(value: Int<0..10>)", 1)
        self.busy.add(("hub", h))

        def restore():
            self.texts[f] = self.texts[f].replace(f"export pure func {h}(value: Int<0..10>)", f"export pure func {h}(value: {old})", 1)
            self.busy.discard(("hub", h))
            return Edit(kind, "restore", f, self.texts[f], f"decl {h}", hub=h)
        return Edit(kind, "open", f, self.texts[f], f"decl {h}", hub=h), restore

    def local_pair(self, kind):
        """`body-error`, `effect`, `ownership`, `obligation`: an error in one unit, restored later."""
        spec = {
            "body-error": ("try func priceOf{g}(", r"Int\.wrappingAdd\(found, (\d+)\)", lambda mt: 'Int.wrappingAdd(found, "e")', "priceOf"),
            "effect": ("func step{g}(", r"  return (Int\.wrappingAdd\(n, 1\))\n", lambda mt: "  return List.of(n).count()\n", "step"),
            "ownership": ("func stock{g}(", r"editable counts\.set\((copy) name, n\)", lambda mt: "editable counts.set(move name, n)", "stock"),
            "obligation": ("func settle{g}(", r"  (Must\.discharge\(move w\))\n", lambda mt: "  discard 0\n", "settle"),
        }[kind]
        for _ in range(50):
            f, m, i = self.group()
            g = f"{m}u{i}"
            unit = f"decl {spec[3]}{g}"
            if (f, unit) in self.busy:
                continue
            mt = self.place(f, spec[0].format(g=g), spec[1])
            if not mt:
                continue
            old = mt.group(0)
            new = spec[2](mt)
            self.put(f, mt.start(), mt.end(), new)
            self.busy.add((f, unit))
            anchor = spec[0].format(g=g)

            def restore(f=f, unit=unit, old=old, new=new, anchor=anchor, m=m, i=i):
                t = self.texts[f]
                a = t.find(anchor)
                b = t.find(new, a)
                self.texts[f] = t[:b] + old + t[b + len(new):]
                self.busy.discard((f, unit))
                return Edit(kind, "restore", f, self.texts[f], unit, module=m, group=i)
            return Edit(kind, "open", f, self.texts[f], unit, module=m, group=i), restore
        return None, None

    def decl(self):
        f, m, _ = self.group(needs=lambda f: f in self.importers)
        self.counter += 1
        name = f"extra{self.cycle}x{self.counter}"
        add = f"\nexport func {name}(n: Int) Int {{\n  return n\n}}\n"
        self.texts[f] = self.texts[f] + add

        def restore():
            t = self.texts[f]
            k = t.rfind(add)
            self.texts[f] = t[:k] + t[k + len(add):]
            return Edit("decl", "restore", f, self.texts[f], f"decl {name}", module=m, name=name)
        return Edit("decl", "open", f, self.texts[f], f"decl {name}", module=m, name=name), restore

    def rename(self):
        # a clampTo member another module imports by name
        for _ in range(50):
            f, m, i = self.group(needs=lambda f: f in self.importers)
            g = f"{m}u{i}"
            users = [u for u in self.importers[f] if f"clampTo{g}" in self.texts[u].split("\n\n")[1]]
            if not users or (f, f"decl clampTo{g}") in self.busy:
                continue
            old, new = f"export pure func clampTo{g}(", f"export pure func clampTo{g}r("
            self.texts[f] = self.texts[f].replace(old, new, 1)
            self.busy.add((f, f"decl clampTo{g}"))

            def restore(f=f, g=g, old=old, new=new, m=m, i=i):
                self.texts[f] = self.texts[f].replace(new, old, 1)
                self.busy.discard((f, f"decl clampTo{g}"))
                return Edit("rename", "restore", f, self.texts[f], f"decl clampTo{g}", module=m, group=i)
            return Edit("rename", "open", f, self.texts[f], f"decl clampTo{g}", module=m, group=i), restore
        return None, None

    def syntax(self):
        for _ in range(50):
            f, m, i = self.group()
            g = f"{m}u{i}"
            unit = f"decl summary{g}"
            if (f, unit) in self.busy:
                continue
            mt = self.place(f, f"func summary{g}(", r"  return sum\n\}\n")
            if not mt:
                continue
            self.put(f, mt.start(), mt.end(), "  return sum\n\n")
            self.busy.add((f, unit))

            def restore(f=f, unit=unit, g=g, m=m, i=i):
                mt = self.place(f, f"func summary{g}(", r"  return sum\n\n")
                self.put(f, mt.start(), mt.end(), "  return sum\n}\n")
                self.busy.discard((f, unit))
                return Edit("syntax", "restore", f, self.texts[f], unit, module=m, group=i)
            return Edit("syntax", "open", f, self.texts[f], unit, module=m, group=i), restore
        return None, None

    def shared_pair(self, kind):
        """`iface`, `contract`: a module's interface or contract, used by the modules importing it first."""
        pool = [f for f in self.first_of if ("shared", f, kind) not in self.busy]
        if not pool:
            return None, None
        f = self.rng.choice(pool)
        m = num(f)
        old, new = {
            "iface": (f"export interface Costed{m} {{\n  func cost(self) Int\n}}", f"export interface Costed{m} {{\n  func cost(self) Int\n  func weight(self) Int\n}}"),
            "contract": (f"export contract Lean{m} {{\n  pure\n}}", f"export contract Lean{m} {{\n  pure\n  noalloc\n}}"),
        }[kind]
        self.texts[f] = self.texts[f].replace(old, new, 1)
        self.busy.add(("shared", f, kind))
        unit = {"iface": f"decl Costed{m}", "contract": f"decl Lean{m}"}[kind]

        def restore():
            self.texts[f] = self.texts[f].replace(new, old, 1)
            self.busy.discard(("shared", f, kind))
            return Edit(kind, "restore", f, self.texts[f], unit, module=m)
        return Edit(kind, "open", f, self.texts[f], unit, module=m), restore

    def alias(self):
        pool = [f for f in self.alias_of if ("alias", f) not in self.busy]
        if not pool:
            return None, None
        f = self.rng.choice(pool)
        m = num(f)
        i = self.rng.choice([1, 4, 7])
        g = f"{m}u{i}"
        old, new = f"export pure func clampTo{g}(value: Int, limit: Int)", f"export pure func clampTo{g}(value: Int, limit: String)"
        self.texts[f] = self.texts[f].replace(old, new, 1)
        self.busy.add(("alias", f))

        def restore():
            self.texts[f] = self.texts[f].replace(new, old, 1)
            self.busy.discard(("alias", f))
            return Edit("alias", "restore", f, self.texts[f], f"decl clampTo{g}", module=m, group=i)
        return Edit("alias", "open", f, self.texts[f], f"decl clampTo{g}", module=m, group=i), restore

    def member(self):
        pool = [k for k in self.member_users if ("member", k) not in self.busy]
        if not pool:
            return None, None
        f, i = self.rng.choice(sorted(pool))
        m = num(f)
        g = f"{m}u{i}"
        old, new = f"export pure func clampTo{g}(value: Int, limit: Int)", f"export pure func clampTo{g}(value: Int, limit: String)"
        self.texts[f] = self.texts[f].replace(old, new, 1)
        self.busy.add(("member", (f, i)))

        def restore():
            self.texts[f] = self.texts[f].replace(new, old, 1)
            self.busy.discard(("member", (f, i)))
            return Edit("member", "restore", f, self.texts[f], f"decl clampTo{g}", module=m, group=i)
        return Edit("member", "open", f, self.texts[f], f"decl clampTo{g}", module=m, group=i), restore

    def chain(self, kind):
        """`probe` (a hub) and `aspect` (a module's aspect): break it, insert a line above it, remove the line,
        restore it. The callers' records must not move with the hub (A1); the aspect's users' records must move with
        the hook, whose positions they carry."""
        if kind == "probe":
            first, restore = self.hub_break("probe")
            if first is None:
                return None
            h = first.meta["hub"]
            f, anchor, unit, meta = first.file, f"export pure func {h}(", f"decl {h}", {"hub": h}
        else:
            pool = [f for f in self.first_of if ("shared", f, "aspect") not in self.busy]
            if not pool:
                return None
            f = self.rng.choice(pool)
            m = num(f)
            self.texts[f] = self.texts[f].replace("      discard Int.wrappingAdd(limit, 1)\n", '      discard Int.wrappingAdd(limit, "x")\n', 1)
            self.busy.add(("shared", f, "aspect"))
            anchor, unit, meta = f"export aspect checked{m}(", f"decl checked{m}", {"module": m}
            first = Edit("aspect", "open", f, self.texts[f], unit, **meta)

            def restore():
                self.texts[f] = self.texts[f].replace('      discard Int.wrappingAdd(limit, "x")\n', "      discard Int.wrappingAdd(limit, 1)\n", 1)
                self.busy.discard(("shared", f, "aspect"))
                return Edit("aspect", "restore", f, self.texts[f], unit, **meta)

        def shift():
            t = self.texts[f]
            a = t.find(anchor)
            self.texts[f] = t[:a] + "// shifted\n" + t[a:]
            return Edit(kind, "shift", f, self.texts[f], unit, **meta)

        def unshift():
            self.texts[f] = self.texts[f].replace("// shifted\n" + anchor, anchor, 1)
            return Edit(kind, "unshift", f, self.texts[f], unit, **meta)
        return first, [shift, unshift, restore]

    def open(self, kind):
        """Apply an opening of `kind`: the edit, and the later steps it schedules."""
        if kind == "body-literal":
            return self.body_literal(), []
        if kind == "body-line":
            return self.body_line(), []
        if kind == "comment":
            return self.comment(), []
        if kind == "sig-compat":
            return self.sig_compat(), []
        if kind == "sig-break":
            e, r = self.hub_break("sig-break")
            return e, [r] if r else []
        if kind in ("body-error", "effect", "ownership", "obligation"):
            e, r = self.local_pair(kind)
            return e, [r] if r else []
        if kind == "decl":
            e, r = self.decl()
            return e, [r]
        if kind == "rename":
            e, r = self.rename()
            return e, [r] if r else []
        if kind == "syntax":
            e, r = self.syntax()
            return e, [r] if r else []
        if kind in ("iface", "contract"):
            e, r = self.shared_pair(kind)
            return e, [r] if r else []
        if kind == "alias":
            e, r = self.alias()
            return e, [r] if r else []
        if kind == "member":
            e, r = self.member()
            return e, [r] if r else []
        if kind in CHAINS:
            got = self.chain(kind)
            if got is None:
                return None, []
            return got
        raise SystemExit(f"unknown kind {kind}")

    def edits(self):
        """The script: every opening of the plan in a seeded order, each pair's later steps 1 to 15 edits after (the
        next edit for `syntax` and `rename`, which leave the package to the resolver alone)."""
        openings = [k for k, n in sorted(self.plan.items()) for _ in range(n)]
        self.rng.shuffle(openings)
        pending = []  # (due, seq, step function)
        step, seq = 0, 0
        while openings or pending:
            pending.sort()
            if pending and (pending[0][0] <= step or not openings):
                _, _, fn = pending.pop(0)
                e = fn()
                step += 1
                yield e
                continue
            # a resolver-only state is restored by the very next edit, before any other opening
            kind = openings.pop()
            e, later = self.open(kind)
            if e is None:
                openings.insert(0, kind)
                if all(not self.can_open(k) for k in set(openings)) and pending:
                    pending[0] = (step, pending[0][1], pending[0][2])
                continue
            step += 1
            yield e
            due = step
            for fn in later:
                seq += 1
                if kind in NEXT:
                    # before anything else that is due: no other edit is made while the resolver answers alone
                    due = -1
                else:
                    due += self.rng.randint(1, 15) if kind not in CHAINS else self.rng.randint(1, 5)
                pending.append((due, seq, fn))

    def can_open(self, kind):
        return kind not in ("sig-compat", "sig-break", "probe") or any(("hub", h) not in self.busy for h in self.hubs)


def soak(pkg_dir, seed, cycles=10, forward=500, plan=None):
    """The soak (design 5): `cycles` cycles, each `forward` edits drawn as the latency run's, then their inverses in
    reverse order, so that each cycle ends at the original texts. Names a `decl` edit adds carry the cycle number."""
    for c in range(cycles):
        s = Script(pkg_dir, seed * 1000 + c, plan=plan or LATENCY, cycle=c)
        last = dict(s.original)
        done = []
        for e in s.edits():
            if len(done) == forward:
                break
            done.append((e, last[e.file]))
            last[e.file] = e.text
            yield c, e
        for e, before in reversed(done):
            yield c, Edit(e.kind, "inverse", e.file, before, e.unit, **e.meta)


if __name__ == "__main__":
    args = sys.argv[1:]
    d = args[0]
    seed = int(args[args.index("--seed") + 1]) if "--seed" in args else 12
    count = int(args[args.index("--count") + 1]) if "--count" in args else 50
    for n, e in enumerate(Script(d, seed).edits()):
        if n >= count:
            break
        print(json.dumps(e.describe(), sort_keys=True))
