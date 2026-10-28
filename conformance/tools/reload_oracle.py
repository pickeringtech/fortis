#!/usr/bin/env python3
"""SP-07's oracle (spikes/SP-07/design.md 11): what a development session must log for a script.

It is written from the design and the edit catalogue, never from the session's code. It has its own model of the
workload (conformance/reload/template.ft):
- each handler is a sequence of steps at virtual times;
- tasks waking at the same time run in the order their invocations were called;
- each version's formulas come from conformance/reload/generate.py's coefficients.

From the script and the catalogue it computes, for every operation:
- each invocation's generation, outcome, value and prints, and the session's own prints (factories and cleanups);
- what a reload must restart, stop or cancel, and which invocations it cancels;
- the live and pinned generations and the generations reclaimed.

The one input it takes from the session is each generation's size in bytes, which the memory cap reads.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "reload"))
import generate  # noqa: E402

CAT = {e["name"]: e for e in generate.catalogue()["edits"]}
RULE_ORDER = ["entry", "state", "di", "dependency"]


class Version:
    """A generation's source: its version number and its edits, with the generation each fresh-identity edit was
    last turned on in (a lambda added again gets a new identity)."""

    def __init__(self, v, edits, fresh=None):
        self.v = v
        self.edits = frozenset(edits)
        self.fresh = dict(fresh or {})

    def has(self, behaviour):
        return any(behaviour in CAT[e].get("behaviour", []) for e in self.edits)


class State:
    def __init__(self, scope, version, args):
        self.scope = scope
        self.version = version
        self.args = args
        self.total = 0
        self.count = 0
        self.spawned = 0
        self.hits = 0
        self.pct = version.has("pct")
        self.key = args.get("key", "") + ("!" if version.has("key_bang") else "")


class Inv:
    def __init__(self, id_, scope, handler, args, gen, version, index, t0, state):
        self.id = id_
        self.scope = scope
        self.handler = handler
        self.args = args
        self.gen = gen
        self.version = version
        self.index = index
        self.t0 = t0
        self.state = state
        self.wake = None
        # a detached child's wake time (spawn): the invocation stays active, and pins its generation, until then
        self.child = None
        self.done = False
        self.outcome = None
        self.value = None
        self.prints = []


class Oracle:
    def __init__(self, sizes):
        self.sizes = sizes
        self.gens = {}
        self.current = None
        self.next_gen = 1
        self.reclaimed = set()
        self.scopes = {}
        self.invs = []
        self.now = 0
        self.caps = (None, None)
        self.ops = []

    # ---- formulas (template.ft, generate.coefficients) --------------------------------------------------------------

    @staticmethod
    def co(version):
        return generate.coefficients(version.v)

    def start_steps(self, inv):
        """What an invocation does when it is called; sets its wake time (None: it completed at once)."""
        v, k, st = inv.version, self.co(inv.version), inv.state
        h, a = inv.handler, inv.args
        if h == "checkout":
            inv.prints.append(f"checkout start @{v.v} {a['cents']}")
            inv.wake = inv.t0 + a["pause"]
        elif h in ("refund", "hit", "receipt"):
            inv.wake = inv.t0 + a["pause"]
        elif h == "spawn":
            inv.child = inv.t0 + a["pause"]
            self.finish(inv, "ok", f"spawn@{v.v}")
        elif h == "spawnDone":
            # the child runs at once; the root sleeps 1 ms, then detaches the finished child
            inv.child = inv.t0
            inv.wake = inv.t0 + 1
        elif h == "quote":
            c, rate = a["cents"], st.args["rate"]
            self.finish(inv, "ok", f"quote@{v.v} r={c * rate + k['R'] + (1 if v.has('rule_bias') else 0)} p={c}")
        elif h == "turn":
            c, rate = a["cents"], st.args["rate"]
            self.finish(inv, "ok", f"turn@{v.v} up={c + rate} down={c - rate}")
        elif h == "fanout":
            # a sleep, then a join of two naps (pause and 1 ms), launched when the sleep ends
            inv.wake = inv.t0 + a["pause"] + max(a["pause"], 1)
        elif h == "late":
            # `late` awaits a future it owns, whose receipt lateValue destroys at its exit, in this generation (D-147:
            # until SP-07's workload changed, it awaited the state's own future, and a second call failed)
            inv.prints.append(f"receipt closed @{v.v}")
            self.finish(inv, "ok", f"shop@{v.v} late={st.args['rate'] + k['L']}")
        elif h == "balance":
            self.finish(inv, "ok", f"shop@{v.v}{'!' if v.has('balance_bang') else ''} total={st.total} count={st.count} spawned={st.spawned}")
        elif h == "owned_total":
            self.finish(inv, "ok", f"shop@{v.v} owned total={st.total}")
        elif h == "bill":
            self.finish(inv, "ok", f"invoice@{v.v} gw:{st.key}:{a['cents']}@{v.v}")
        else:
            raise ValueError(h)

    def wake_steps(self, inv):
        v, k, st, a = inv.version, self.co(inv.version), inv.state, inv.args
        if inv.handler == "checkout":
            c, rate = a["cents"], st.args["rate"]
            pa = c * k["A"] + k["B"]
            pb = c * rate + k["R"] + (1 if v.has("rule_bias") else 0)
            pc = c + rate * (4 if v.has("bonus4") else 2)
            ps = c * (rate + k["S"] + (1 if v.has("scale1") else 0))
            pd = c - 10 if st.pct else c - (1 + k["F"])
            pg = 2 * (c + k["C"])
            prior, n = st.total, st.count
            st.total += pa
            st.count += 1
            pe = c * k["T"]
            inv.prints.append(f"checkout end @{v.v} {c}")
            self.finish(inv, "ok", f"shop@{v.v} a={pa} b={pb} c={pc} s={ps} d={pd} e={pe} g={pg} prior={prior} n={n} f=0")
        elif inv.handler == "refund":
            c = a["cents"]
            limit = 200 if v.has("limit200") else 100
            if c > limit:
                self.finish(inv, "failure", f"shop.declined{{marker=@{v.v}, cents={c}}} \"declined ${{cents}} ${{marker}}\"")
            else:
                prior = st.total
                st.total -= c
                self.finish(inv, "ok", f"refund@{v.v} prior={prior}")
        elif inv.handler == "hit":
            n = a["n"]
            w = n * k["W"] + n + k["C"]
            prior = st.hits
            st.hits += w
            self.finish(inv, "ok", f"meter@{v.v} w={w} prior={prior}")
        elif inv.handler == "fanout":
            self.finish(inv, "ok", f"shop@{v.v} fanout")
        elif inv.handler == "spawnDone":
            self.finish(inv, "ok", f"spawn@{v.v}")
        elif inv.handler == "receipt":
            # the result's cleanup runs in the invocation's own generation, when the session takes the result
            inv.prints.append(f"receipt closed @{v.v}")
            self.finish(inv, "ok", "Receipt(out: Console, cents: 5)")

    def finish(self, inv, outcome, value):
        inv.done = True
        inv.outcome = outcome
        inv.value = value

    def cancel(self, inv):
        if not inv.done:
            if inv.handler == "checkout":
                inv.prints.append(f"checkout end @{inv.version.v} {inv.args['cents']}")
            self.finish(inv, "cancelled", "")
        inv.child = None

    # ---- the scheduler -----------------------------------------------------------------------------------------------

    def pending(self):
        """The active invocations: unfinished, or with a detached child still to run."""
        return [i for i in self.invs if not i.done or i.child is not None]

    def run_to(self, t):
        while True:
            # tasks run in launch order at equal times: an invocation's root, then the child it detached at its call
            due = sorted([(i.wake, i.index, 0, i) for i in self.invs if not i.done and i.wake is not None and i.wake <= t]
                         + [(i.child, i.index, 1, i) for i in self.invs if i.child is not None and i.child <= t], key=lambda x: x[:3])
            if not due:
                break
            when, _, child, i = due[0]
            self.now = max(self.now, when)
            if child:
                i.state.spawned += 1
                i.child = None
            else:
                self.wake_steps(i)
        self.now = max(self.now, t)

    # ---- scopes ------------------------------------------------------------------------------------------------------

    def build(self, scope, factory, args, gen, out):
        version = self.gens[gen]
        missing = self.recipe_problem(scope, version)
        if missing:
            return None
        if scope == "shop":
            out["session_prints"].append(f"open shop @{version.v}")
        elif scope == "meter":
            out["session_prints"].append(f"open meter @{version.v}")
        out["started"].append((scope, gen))
        return State(scope, version, args)

    def recipe_problem(self, scope, version):
        if scope == "meter" and "meter-factory-param" in version.edits:
            return "openMeter needs scale"
        if scope == "billing" and "billing-factory-removed" in version.edits:
            return "openBilling is gone"
        return None

    def release(self, scope, gen, out):
        s = self.scopes[scope]
        if s["state"] is None:
            return
        if scope in ("shop", "meter"):
            out["session_prints"].append(f"audit {scope} closed @{self.gens[gen].v}")
        if scope == "shop":
            # the state's unawaited future: its receipt goes with the state
            out["session_prints"].append(f"receipt closed @{self.gens[gen].v}")
        s["state"] = None

    # ---- the operations ----------------------------------------------------------------------------------------------

    def op(self, op):
        out = {"op": op["op"], "session_prints": [], "started": [], "actions": {}, "cancelled": {}, "completed": [], "reclaimed": [], "rejected": False, "snapshot": None, "stopped": []}
        before = {i.id for i in self.invs if i.done}
        name = op["op"]
        if name == "open":
            v, edits = op["_version"]
            self.gens[1] = Version(v, edits)
            self.current = 1
            self.next_gen = 2
        elif name == "start":
            st = self.build(op["scope"], op["factory"], op.get("args", {}), self.current, out)
            self.scopes[op["scope"]] = {"factory": op["factory"], "args": op.get("args", {}), "state": st, "stopped": False}
        elif name == "call":
            s = self.scopes.get(op["scope"])
            if s is None or s["stopped"] or s["state"] is None:
                out["error"] = op["id"]
            else:
                inv = Inv(op["id"], op["scope"], op["handler"], op.get("args", {}), self.current, self.gens[self.current], len(self.invs), self.now, s["state"])
                self.invs.append(inv)
                self.start_steps(inv)
                # a call settles: whatever can run at the current time runs before the next operation
                self.run_to(self.now)
        elif name == "run":
            self.run_to(op["until"])
        elif name == "reload":
            v, edits = op["_version"]
            if any(CAT[e].get("rejected") for e in edits):
                out["rejected"] = True
            else:
                self.reload(Version(v, edits), out)
        elif name == "restart":
            self.restart_now({op["scope"]: "requested"}, out)
        elif name == "caps":
            self.caps = (op.get("generations"), op.get("bytes"))
            plans = self.evictions(None, set())
            if plans:
                self.restart_now(plans, out)
        elif name == "snapshot":
            out["snapshot"] = self.snapshot()
        elif name == "finish":
            self.run_to(10 ** 12)
            out["snapshot"] = self.snapshot()
        out["completed"] = [i.id for i in self.invs if i.done and i.id not in before]
        for g in sorted(self.gens):
            if g != self.current and g not in self.reclaimed and not any(i.gen == g for i in self.pending()):
                self.reclaimed.add(g)
                out["reclaimed"].append(g)
        if name == "snapshot" or name == "finish":
            out["snapshot"] = self.snapshot()
        self.ops.append(out)
        return out

    def snapshot(self):
        pinned = sorted({i.gen for i in self.pending()})
        live = sorted(set(pinned) | {self.current})
        return {"current": self.current, "live": live, "pinned": pinned}

    def transition(self, old, new):
        """The scopes a reload from `old` to `new` restarts (scope -> rule) and the scopes whose old readers it
        cancels, from the catalogue."""
        added = new.edits - old.edits
        removed = old.edits - new.edits
        acts = {}
        for e in added:
            for s, r in CAT[e]["on_add"].items():
                acts.setdefault(s, set()).add(r)
        for e in removed:
            for s, r in CAT[e]["on_remove"].items():
                acts.setdefault(s, set()).add(r)
        rules = {s: min(rs, key=RULE_ORDER.index) for s, rs in acts.items()}
        readers = {}
        for e in added:
            for s in CAT[e].get("old_readers", []):
                readers.setdefault(s, []).append(e)
        return rules, readers

    def reload(self, new, out):
        cur = self.gens[self.current]
        gid = self.next_gen
        for e in new.edits:
            if CAT[e].get("fresh_identity"):
                new.fresh[e] = cur.fresh.get(e, gid) if e in cur.edits else gid
        rules, readers = self.transition(cur, new)
        plans = {}
        for s, r in rules.items():
            if s in self.scopes and not self.scopes[s]["stopped"]:
                plans[s] = r
        # R2: an old reader of a scope that is not restarted
        old_readers = {}
        for s, eds in readers.items():
            if s in plans or s not in self.scopes or self.scopes[s]["stopped"]:
                continue
            gens = set()
            for i in self.pending():
                if i.scope != s:
                    continue
                gv = self.gens[i.gen]
                for e in eds:
                    if e not in gv.edits or (CAT[e].get("fresh_identity") and gv.fresh.get(e) != new.fresh.get(e)):
                        gens.add(i.gen)
            if gens:
                old_readers[s] = gens
        cancelled = {i.id for i in self.pending() if i.scope in plans or (i.scope in old_readers and i.gen in old_readers[i.scope])}
        for s, r in self.evictions(gid, cancelled).items():
            plans.setdefault(s, r)
        self.gens[gid] = new
        self.next_gen = gid + 1
        self.apply(plans, old_readers, out, new_gen=gid)

    def evictions(self, new_gen, cancelled):
        g_cap, b_cap = self.caps
        plans = {}
        gone = set(cancelled)
        head = new_gen if new_gen is not None else self.current
        while True:
            pinned = sorted({i.gen for i in self.pending() if i.id not in gone and i.gen != head})
            count = len(pinned) + 1
            size = sum(self.sizes.get(g, 0) for g in pinned)
            if g_cap is not None and count > g_cap:
                rule = "generation-cap"
            elif b_cap is not None and size > b_cap:
                rule = "memory-cap"
            else:
                break
            oldest = pinned[0]
            for s in sorted({i.scope for i in self.pending() if i.id not in gone and i.gen == oldest}):
                for i in self.pending():
                    if i.scope == s:
                        gone.add(i.id)
                plans[s] = rule
        return plans

    def apply(self, plans, old_readers, out, new_gen=None):
        rel_gen = self.current
        for s in sorted(plans):
            ids = [i.id for i in self.pending() if i.scope == s]
            out["cancelled"][s] = ids
            for i in self.pending():
                if i.scope == s:
                    self.cancel(i)
        for s in sorted(old_readers):
            ids = [i.id for i in self.pending() if i.scope == s and i.gen in old_readers[s]]
            out["cancelled"][s] = ids
            for i in self.pending():
                if i.scope == s and i.gen in old_readers[s]:
                    self.cancel(i)
            out["actions"][s] = "old-reader"
        for s in sorted(plans):
            self.release(s, rel_gen, out)
        if new_gen is not None:
            self.current = new_gen
        for s in sorted(plans):
            rule = plans[s]
            sc = self.scopes[s]
            if rule == "entry" or self.recipe_problem(s, self.gens[self.current]):
                sc["stopped"] = True
                out["actions"][s] = "entry"
                out["stopped"].append(s)
                continue
            out["actions"][s] = rule
            sc["state"] = self.build(s, sc["factory"], sc["args"], self.current, out)

    def restart_now(self, plans, out):
        self.apply(plans, {}, out)
