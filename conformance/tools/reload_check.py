#!/usr/bin/env python3
"""SP-07's harness (SP-07's design, sections 11 and 12): run development sessions over the reload workload and check
them against the oracle (reload_oracle.py) and the normative assertions.

    python3 conformance/tools/reload_check.py --json OUT [--seed N] [--soak RELOADS] [--fortisc PATH]
        [--fault-fortisc PATH] [--fault NAME]

Every check belongs to a family of design section 12. A scenario that raises records `<FAMILY>.exception` for each
family it covers. A family with no check records `<FAMILY>.incomplete`. The results file says whether the run is
complete. With `--fault`, every session runs with FORTIS_RELOAD_FAULT set: the apparatus check
(reload_mutations.py).
"""
import argparse
import fnmatch
import json
import os
import random
import re
import subprocess
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "conformance", "reload"))
sys.path.insert(0, HERE)
import generate  # noqa: E402
import reload_oracle  # noqa: E402

FAMILIES = ["I1", "I2", "I3", "I4", "I5", "M", "C", "R-a", "R-b", "R-c", "X", "G1", "G2", "G3", "G4", "S", "J", "D", "K1", "K2", "N1", "N2", "N3", "N4", "N5"]
CAT = {e["name"]: e for e in generate.catalogue()["edits"]}
# the functions whose bodies carry a version coefficient: a version bump changes exactly these (template.ft)
BUMP = ["main::cleanup Audit", "main::method Flat.apply", "main::function shopMark", "main::function price", "main::function common", "main::function lateValue", "main::function tally", "main::function nap", "main::function openShop", "main::function openShop/λ1", "main::function openShop/λ3", "main::function checkout", "main::function refund", "main::function meterMark", "main::function weight", "main::function openMeter", "main::method Gateway.charge", "main::method Invoicer.bill", "main::cleanup Receipt", "main::function quote", "main::function turn", "main::function spawn", "main::function spawnDone"]


class SessionCrash(Exception):
    """The system under test failed: a failed check, not a broken harness."""


class Results:
    def __init__(self):
        self.checks = []

    def add(self, family, name, ok, detail=""):
        self.checks.append({"family": family, "name": f"{family}.{name}", "pass": bool(ok), "detail": detail if not ok else ""})


# ---- scripts -------------------------------------------------------------------------------------------------------

def o_open(v, edits=()):
    return {"op": "open", "_version": (v, sorted(edits))}


def o_reload(v, edits=()):
    return {"op": "reload", "_version": (v, sorted(edits))}


def o_call(scope, handler, id_, **args):
    return {"op": "call", "scope": scope, "handler": handler, "id": id_, "args": args}


def o_run(t):
    return {"op": "run", "until": t}


def starts(rate=3, key="k1"):
    return [{"op": "start", "scope": "shop", "factory": "openShop", "args": {"rate": rate}},
            {"op": "start", "scope": "meter", "factory": "openMeter"},
            {"op": "start", "scope": "billing", "factory": "openBilling", "args": {"key": key}}]


def scenarios(seed, soak):
    out = {}
    out["isolation"] = [o_open(1)] + starts() + [
        o_call("shop", "checkout", "c1", cents=10, pause=50), o_call("shop", "refund", "r1", cents=500, pause=60),
        o_call("shop", "fanout", "f1", pause=300), o_call("meter", "hit", "h1", n=4, pause=40), o_call("billing", "bill", "b1", cents=7),
        o_call("shop", "receipt", "rc1", pause=40), o_call("shop", "spawn", "sp1", pause=45), o_call("shop", "quote", "qt1", cents=6), o_call("shop", "turn", "tn1", cents=6),
        o_run(10), o_reload(2),
        o_call("shop", "checkout", "c2", cents=11, pause=20), o_call("shop", "late", "l2"), o_call("shop", "balance", "bal2"),
        o_call("meter", "hit", "h2", n=5, pause=15), {"op": "snapshot"}, o_run(35), o_reload(3),
        o_call("shop", "checkout", "c3", cents=12, pause=5), o_call("billing", "bill", "b3", cents=8), {"op": "snapshot"},
        o_run(55), {"op": "snapshot"}, o_run(400), {"op": "snapshot"}, o_call("shop", "owned_total", "o3"), {"op": "finish"}]
    out["future-after-reclaim"] = [o_open(1)] + starts() + [o_reload(2), o_reload(3), {"op": "snapshot"}, o_call("shop", "late", "l3"), o_call("shop", "late", "l3b"), {"op": "finish"}]
    out["bump"] = [o_open(1)] + starts() + [o_call("shop", "checkout", "k1", cents=10, pause=50), o_run(10), o_reload(2), o_call("shop", "checkout", "k2", cents=10, pause=5), {"op": "finish"}]
    for name, e in CAT.items():
        pre = list(e.get("requires", []))
        out["edit-" + name] = [o_open(1, pre)] + starts() + [
            o_call("shop", "checkout", "a", cents=10, pause=50), o_call("shop", "refund", "r", cents=20, pause=50),
            o_call("meter", "hit", "h", n=4, pause=50), o_call("billing", "bill", "b", cents=7), o_run(10),
            o_reload(1, pre + [name]),
            o_call("shop", "checkout", "a2", cents=10, pause=5), o_call("shop", "balance", "bal", ), o_call("shop", "owned_total", "own"),
            o_call("shop", "quote", "qt", cents=4), o_call("shop", "turn", "tn", cents=4),
            o_call("meter", "hit", "h2", n=3, pause=5), o_call("billing", "bill", "b2", cents=9), o_run(100), {"op": "finish"}]
    out["detached"] = [o_open(1)] + starts() + [o_call("shop", "spawn", "d1", pause=100), o_call("shop", "spawn", "d2", pause=20), o_call("shop", "spawnDone", "d3"), o_run(10), o_reload(2), {"op": "snapshot"},
                                               o_run(30), {"op": "snapshot"}, o_call("shop", "balance", "db1"), {"op": "restart", "scope": "shop"}, {"op": "snapshot"},
                                               o_call("shop", "balance", "db2"), {"op": "finish"}]
    out["detached-caps"] = [o_open(1)] + starts() + [{"op": "caps", "generations": 1}, o_call("shop", "spawn", "e1", pause=1000), o_reload(2), {"op": "snapshot"},
                                                    o_call("shop", "balance", "eb"), {"op": "finish"}]
    out["three-gen"] = [o_open(1)] + starts() + [o_call("shop", "checkout", "t1", cents=10, pause=5), o_run(10), o_reload(1, ["double-lambda"]),
                                                o_call("shop", "checkout", "t2", cents=10, pause=5), o_run(20), o_reload(1, ["double-lambda", "remove-twice"]),
                                                o_call("shop", "checkout", "t3", cents=10, pause=5), {"op": "finish"}]
    out["backflow"] = [o_open(1)] + starts() + [o_call("shop", "checkout", "k", cents=10, pause=100), o_call("meter", "hit", "m", n=2, pause=100), o_run(10),
                                               o_reload(1, ["hooks-backflow"]), o_call("shop", "checkout", "k2", cents=10, pause=5), {"op": "finish"}]
    out["constant"] = [o_open(1)] + starts() + [o_call("shop", "refund", "q1", cents=150, pause=0), o_reload(1, ["limit-constant"]),
                                               o_call("shop", "refund", "q2", cents=150, pause=0), o_call("shop", "refund", "q3", cents=250, pause=0), {"op": "finish"}]
    out["recipe"] = [o_open(1)] + starts() + [o_call("meter", "hit", "m1", n=1, pause=50), o_run(10), o_reload(1, ["meter-factory-param"]), o_call("meter", "hit", "m2", n=1, pause=1),
                                             o_reload(1, ["meter-factory-param", "billing-factory-removed"]), o_call("billing", "bill", "b2", cents=1),
                                             o_call("shop", "balance", "s2"), {"op": "finish"}]
    out["restart"] = [o_open(1)] + starts() + [o_call("shop", "checkout", "x1", cents=10, pause=50), o_run(10), {"op": "restart", "scope": "shop"},
                                              o_call("shop", "balance", "x2"), {"op": "finish"}]
    out["caps-g2"] = [o_open(1)] + starts() + [{"op": "caps", "generations": 2}, o_call("meter", "hit", "p1", n=1, pause=1000), {"op": "snapshot"}, o_reload(2),
                                              o_call("shop", "fanout", "p2", pause=1000), {"op": "snapshot"}, o_reload(3), {"op": "snapshot"},
                                              o_call("shop", "checkout", "p3", cents=1, pause=1000), o_reload(4), {"op": "snapshot"}, {"op": "finish"}]
    out["caps-g1"] = [o_open(1)] + starts() + [{"op": "caps", "generations": 1}, o_call("shop", "fanout", "q1", pause=1000), o_reload(2), {"op": "snapshot"},
                                              o_call("meter", "hit", "q2", n=1, pause=1000), o_reload(3), {"op": "snapshot"}, {"op": "finish"}]
    out["caps-lower"] = [o_open(1)] + starts() + [o_call("meter", "hit", "w1", n=1, pause=1000), o_reload(2), o_call("shop", "fanout", "w2", pause=1000), o_reload(3),
                                                 o_call("shop", "checkout", "w3", cents=1, pause=1000), {"op": "snapshot"}, {"op": "caps", "generations": 2}, {"op": "snapshot"}, {"op": "finish"}]
    out["caps-bytes"] = [o_open(1)] + starts() + [{"op": "caps", "_bytes": 1.5}, o_call("meter", "hit", "y1", n=1, pause=1000), o_reload(2),
                                                 o_call("shop", "fanout", "y2", pause=1000), {"op": "snapshot"}, o_reload(3), {"op": "snapshot"}, {"op": "finish"}]
    out["soak"] = soak_script(seed, soak)
    return out


SOAK_EDITS = sorted(n for n, e in CAT.items() if e.get("soak"))
SOAK_CALLS = [("shop", "checkout"), ("shop", "refund"), ("shop", "fanout"), ("shop", "balance"), ("shop", "owned_total"), ("shop", "late"), ("shop", "quote"), ("shop", "turn"),
              ("shop", "spawn"), ("shop", "spawnDone"), ("shop", "receipt"), ("meter", "hit"), ("billing", "bill")]


def soak_script(seed, reloads):
    rng = random.Random(seed)
    ops = [o_open(1)] + starts() + [{"op": "caps", "generations": 3}]
    v, edits, t, k = 1, set(), 0, 0
    for r in range(reloads):
        for _ in range(rng.randint(0, 3)):
            k += 1
            scope, h = rng.choice(SOAK_CALLS)
            args = {"checkout": {"cents": rng.randint(1, 50), "pause": rng.randint(0, 300)}, "refund": {"cents": rng.choice([5, 150, 250]), "pause": rng.randint(0, 300)},
                    "fanout": {"pause": rng.randint(0, 300)}, "hit": {"n": rng.randint(1, 9), "pause": rng.randint(0, 300)}, "bill": {"cents": rng.randint(1, 9)},
                    "quote": {"cents": rng.randint(1, 9)}, "turn": {"cents": rng.randint(1, 9)}, "spawn": {"pause": rng.randint(0, 300)}, "receipt": {"pause": rng.randint(0, 300)}}.get(h, {})
            ops.append(o_call(scope, h, f"s{k}", **args))
        t += rng.randint(0, 150)
        ops.append(o_run(t))
        if rng.random() < 0.5:
            v += 1
        if rng.random() < 0.6:
            edits ^= {rng.choice(SOAK_EDITS)}
        ops.append(o_reload(v, sorted(edits)))
        ops.append({"op": "snapshot", "_reload": r + 1})
    ops.append({"op": "finish"})
    return ops


# ---- running -------------------------------------------------------------------------------------------------------

class Runner:
    def __init__(self, fortisc, work, fault=None):
        self.fortisc = fortisc
        self.work = work
        self.fault = fault
        os.makedirs(work, exist_ok=True)

    def materialise(self, name, ops, size=None):
        """Write the versions and the script; return the script's path and the ops as the session reads them."""
        d = os.path.join(self.work, name)
        os.makedirs(d, exist_ok=True)
        real = []
        for op in ops:
            op = dict(op)
            if "_version" in op:
                v, edits = op["_version"]
                fname = generate.name(v, edits)
                # always written: a version is a function of the template and the catalogue, which may have changed
                with open(os.path.join(d, fname), "w", encoding="utf-8") as f:
                    f.write(generate.text(v, edits))
                op["source"] = fname
            if "_bytes" in op:
                op["bytes"] = int(op["_bytes"] * CAP_UNIT[0])
            real.append({k: v for k, v in op.items() if not k.startswith("_")})
        script = os.path.join(d, "script.json")
        with open(script, "w", encoding="utf-8") as f:
            json.dump(real, f, indent=1, ensure_ascii=False)
        return script, real

    def run(self, name, ops, size=None, tag=""):
        script, real = self.materialise(name, ops, size)
        d = os.path.dirname(script)
        ev, me, fr = os.path.join(d, f"events{tag}.jsonl"), os.path.join(d, f"metrics{tag}.jsonl"), os.path.join(d, f"frames{tag}.jsonl")
        env = dict(os.environ)
        env.pop("FORTIS_RELOAD_FAULT", None)
        if self.fault:
            env["FORTIS_RELOAD_FAULT"] = self.fault
        if not os.path.exists(self.fortisc):
            raise FileNotFoundError(f"no fortisc at {self.fortisc}")
        p = subprocess.run([self.fortisc, "dev", "--script", script, "--events", ev, "--metrics", me, "--frames", fr], capture_output=True, text=True, timeout=1800, env=env)
        if p.returncode == 2:
            # an invocation or host failure (the script, the event log, a file): not the system under test's failing
            raise RuntimeError(f"fortisc dev could not run the session (exit 2): {p.stderr[-2000:]}")
        if p.returncode != 0:
            raise SessionCrash(f"fortisc dev exited {p.returncode}: {p.stderr[-2000:]}")
        with open(ev, encoding="utf-8") as f:
            text = f.read()
        events = [json.loads(l) for l in text.splitlines() if l.strip()]
        metrics = []
        if os.path.exists(me):
            with open(me, encoding="utf-8") as f:
                metrics = [json.loads(l) for l in f if l.strip()]
        self.frames = []
        if os.path.exists(fr):
            with open(fr, encoding="utf-8") as f:
                self.frames = [json.loads(l) for l in f if l.strip()]
        return events, metrics, text, real

    def sizes(self, name, versions):
        """Each version's IR size measured apart from the session (`fortisc dev --ir-size`), for G3."""
        d = os.path.join(self.work, name)
        os.makedirs(d, exist_ok=True)
        files = []
        for v, edits in versions:
            p = os.path.join(d, generate.name(v, edits))
            with open(p, "w", encoding="utf-8") as f:
                f.write(generate.text(v, edits))
            files.append(p)
        out = subprocess.run([self.fortisc, "dev", "--ir-size"] + files, capture_output=True, text=True, timeout=600).stdout
        sizes = {}
        for line in out.splitlines():
            n, f = line.split(" ", 1)
            sizes[os.path.basename(f)] = int(n)
        return sizes


def split(events):
    ops = []
    for e in events:
        if e["event"] == "op":
            ops.append({"index": e["index"], "op": e["op"], "events": []})
        elif ops:
            ops[-1]["events"].append(e)
    return ops


def simulate(ops, sizes):
    o = reload_oracle.Oracle(sizes)
    outs = []
    for op in ops:
        op2 = dict(op)
        if "_bytes" in op2:
            op2["bytes"] = int(op2["_bytes"] * CAP_UNIT[0])
        outs.append(o.op(op2))
    return o, outs


# ---- source index for explanation paths ---------------------------------------------------------------------------

def function_bodies(text):
    """Each function's and constant's text in a version's source, by name: its declaration line and what follows it
    up to the next declaration at its own indentation or less."""
    out = {}
    lines = text.splitlines()
    for k, line in enumerate(lines):
        m = re.match(r"(\s*)(?:export\s+)?(?:(?:pure|async|try|recovers)\s+)*(?:func|const)\s+(\w+)", line)
        if not m:
            continue
        ind = len(m.group(1))
        body = [line]
        for nxt in lines[k + 1:]:
            if nxt.strip() and len(nxt) - len(nxt.lstrip()) <= ind and not nxt.strip().startswith("}"):
                break
            body.append(nxt)
        out.setdefault(m.group(2), "\n".join(body))
    return out


def source_index(text):
    """class and enum names with their fields (`T.f`, `T.V.f`), and function names, of a version's source. A DI
    context's generated classes (`C`, `C<Level>`) and their members exist only in the elaborated text: every name
    that starts with a context's name is accepted."""
    fields, funcs = set(), set()
    contexts = re.findall(r"^context (\w+)", text, re.M)
    cur = None
    for line in text.splitlines():
        m = re.match(r"\s*(?:export\s+)?(?:class|enum|error)\s+(\w+)", line)
        if m:
            cur = m.group(1)
            continue
        if line.startswith("}"):
            cur = None
            continue
        if cur:
            fm = re.match(r"\s+(\w+):\s", line)
            if fm:
                fields.add(f"{cur}.{fm.group(1)}")
            vm = re.match(r"\s+(\w+)\((.*)\)\s*$", line)
            if vm:
                for part in vm.group(2).split(","):
                    if ":" in part:
                        fields.add(f"{cur}.{vm.group(1)}.{part.split(':')[0].strip()}")
            pm = re.match(r"\s+payload \{(.*)\}", line)
            if pm:
                for part in pm.group(1).split(","):
                    fields.add(f"{cur}.{part.split(':')[0].strip()}")
        for fm in re.finditer(r"func\s+(\w+)", line):
            funcs.add(fm.group(1))
    return Names(fields, contexts), Names(funcs, contexts)


class Names(set):
    def __init__(self, items, contexts):
        super().__init__(items)
        self.contexts = contexts

    def __contains__(self, x):
        return set.__contains__(self, x) or any(x.startswith(c) for c in self.contexts)


# ---- the checks ----------------------------------------------------------------------------------------------------

STATE_TYPES = {"shop": "Shared[Shop]", "meter": "Shared[Meter]", "billing": "Shared[BillingApplication]"}
FACTORIES = {"shop": "openShop", "meter": "openMeter", "billing": "openBilling"}


def check_scenario(res, name, ops, events, metrics, runner, caps=None, coarse=False, ir_sizes=None):
    actual = split(events)
    sizes = {e["gen"]: e["bytes"] for e in events if e["event"] == "generation"}
    if ir_sizes:
        # the oracle counts sizes measured apart from the session: generation k is the k-th version installed
        vs = [generate.name(*op["_version"]) for op in ops if "_version" in op and not any(CAT[x].get("rejected") for x in op["_version"][1])]
        sizes = {k + 1: ir_sizes[n] for k, n in enumerate(vs)}
    oracle, outs = simulate(ops, sizes)
    if len(actual) != len(ops):
        res.add("I2", f"{name}.ops", False, f"{len(actual)} operations logged for {len(ops)}")
    # per invocation
    inv_ev = {e["id"]: e for e in events if e["event"] == "invocation"}
    comp = {e["id"]: e for e in events if e["event"] == "completed"}
    prints = {}
    for e in events:
        if e["event"] == "print" and e["id"] != "session":
            prints.setdefault(e["id"], []).append((e["gen"], e["text"]))
    bad = {"I1": [], "I2": [], "I3": [], "I4": [], "I5": [], "M": [], "N1": []}
    counted = {k: 0 for k in bad}
    for inv in oracle.invs:
        a = comp.get(inv.id)
        exp_v = inv.version.v
        got = (a or {}).get("gen"), (a or {}).get("outcome"), (a or {}).get("value")
        exp = (inv.gen, inv.outcome, inv.value if inv.outcome != "error" else got[2])
        texts = [t for _, t in prints.get(inv.id, [])]
        markers = set(re.findall(r"@(\d+)", (got[2] or "") + " ".join(texts)))
        want = set(re.findall(r"@(\d+)", (exp[2] or "") + " ".join(inv.prints)))
        counted["I1"] += 1
        if got[0] != inv.gen or markers != want or want - {str(exp_v)} or texts != inv.prints or any(g != inv.gen for g, _ in prints.get(inv.id, [])):
            bad["I1"].append(f"{inv.id}: gen {got[0]} (expected {inv.gen}), markers {sorted(markers)} (expected @{exp_v}), prints {texts} (expected {inv.prints})")
        counted["I2"] += 1
        if got[1:] != exp[1:]:
            bad["I2"].append(f"{inv.id}: {got[1]} {got[2]!r} (expected {exp[1]} {exp[2]!r})")
        carried = inv.state.version is not inv.version
        if inv.handler == "checkout" and carried and inv.outcome == "ok":
            counted["I3"] += 1
            pick = lambda s: re.findall(r" (b|c|s)=(-?\d+)", s or "")
            if pick(got[2]) != pick(exp[2]):
                bad["I3"].append(f"{inv.id}: {pick(got[2])} (expected {pick(exp[2])})")
        if inv.handler == "late":
            counted["I4"] += 1
            if got[1:] != exp[1:]:
                bad["I4"].append(f"{inv.id}: {got[2]!r} (expected {exp[2]!r})")
        if inv.handler == "refund" and name == "constant":
            counted["I5"] += 1
            if got[1:] != exp[1:]:
                bad["I5"].append(f"{inv.id}: {got[1]} {got[2]!r} (expected {exp[1]} {exp[2]!r})")
        if carried and inv.outcome in ("ok", "failure"):
            counted["M"] += 1
            if got[1:] != exp[1:]:
                bad["M"].append(f"{inv.id}: {got[2]!r} (expected {exp[2]!r})")
        counted["N1"] += 1
        ie = inv_ev.get(inv.id)
        pg = {g for g, _ in prints.get(inv.id, [])}
        if ie is None or (a and a["gen"] != ie["gen"]) or (pg and pg != {ie["gen"]}):
            bad["N1"].append(f"{inv.id}: invocation gen {(ie or {}).get('gen')}, completed gen {got[0]}, print gens {sorted(pg)}")
    # N1: the frame log exists and shows each completed invocation's handler, and a fanout's two naps
    counted["N1"] += 1
    by_id = {}
    for fr in runner.frames:
        by_id.setdefault(fr["id"], []).append(fr["ident"])
    for inv in oracle.invs:
        if inv.outcome in ("ok", "failure"):
            got_f = by_id.get(inv.id, [])
            if f"main::function {inv.handler}" not in got_f:
                bad["N1"].append(f"no frame of {inv.id}'s handler {inv.handler} in the frame log")
            if inv.handler == "fanout" and got_f.count("main::function nap") < 2:
                bad["N1"].append(f"{inv.id}'s naps are missing from the frame log")
    # N1: every frame the session ran belongs to its root invocation's generation (the frame log)
    for fr in runner.frames:
        counted["N1"] += 1
        m = re.match(r"\w+#factory@(\d+)$", fr["id"])
        want_gen = int(m.group(1)) if m else (inv_ev[fr["id"]]["gen"] if fr["id"] in inv_ev else None)
        if fr["id"] != "session" and fr["gen"] != want_gen:
            bad["N1"].append(f"frame {fr['ident']} of {fr['id']} ran generation {fr['gen']} (expected {want_gen})")
    for fam, b in bad.items():
        if counted[fam]:
            res.add(fam, name, not b, "; ".join(b[:5]))
    # per operation
    ra, rb, rc, xx, g1, g2, n2, n3, n4, n5, g3 = ([] for _ in range(11))
    stopped = set()
    versions = {}
    vi = 0
    for i, (op, act, exp) in enumerate(zip(ops, actual, outs)):
        evs = act["events"]
        if "_version" in op:
            versions[i] = op["_version"]
        acts = {}
        cancelled = {}
        for e in evs:
            if e["event"] == "restart":
                acts[e["scope"]] = e["rule"]
            elif e["event"] == "stopped":
                acts[e["scope"]] = "entry" if e["rule"] in ("entry", "rebuild") else e["rule"]
            elif e["event"] == "old-reader":
                acts[e["scope"]] = "old-reader"
            if e["event"] in ("restart", "stopped", "old-reader"):
                cancelled[e["scope"]] = sorted(e.get("cancelled", []))
        raw_acts = dict(acts)
        if coarse:
            # edits interact in the soak: which rule fires first depends on the others, the kind of action does not
            kind = lambda r: r if r in ("entry", "old-reader") else "restart"
            acts = {s: kind(r) for s, r in acts.items()}
            exp = dict(exp, actions={s: kind(r) for s, r in exp["actions"].items()})
        if op["op"] in ("reload", "restart", "caps") and acts != exp["actions"]:
            ra.append(f"op {i} {op['op']} {op.get('_version', '')}: {acts} (expected {exp['actions']})")
        exp_cancel = {s: sorted(v) for s, v in exp["cancelled"].items()}
        if op["op"] in ("reload", "restart", "caps") and {s: v for s, v in cancelled.items()} != exp_cancel:
            rb.append(f"op {i}: cancelled {cancelled} (expected {exp_cancel})")
        # ordering: cancelled invocations' prints, then the old state's cleanup, then the rebuild
        pos = {}
        for k, e in enumerate(evs):
            if e["event"] == "print" and e["id"] == "session":
                m = re.match(r"audit (\w+) closed @(\d+)", e["text"])
                if m:
                    pos.setdefault(("audit", m.group(1)), []).append((k, e["gen"]))
            if e["event"] == "print" and e["id"] != "session":
                pos.setdefault(("print", e["id"]), []).append((k, e["gen"]))
            if e["event"] == "started":
                pos.setdefault(("started", e["scope"]), []).append((k, e["gen"]))
        for k, e in enumerate(evs):
            if e["event"] == "release":
                pos.setdefault(("release", e["scope"]), []).append((k, e["gen"]))
            if e["event"] == "completed":
                pos.setdefault(("completed", e["id"]), []).append((k, e["gen"]))
            if e["event"] == "state-retained":
                rb.append(f"op {i}: {e['scope']}'s state was still held after the drain")
        sig = {e["scope"]: (k, e["tasks"]) for k, e in enumerate(evs) if e["event"] == "signal"}
        dra = {e["scope"]: (k, e["done"], e["stalled"]) for k, e in enumerate(evs) if e["event"] == "drained"}
        gen_at = [k for k, e in enumerate(evs) if e["event"] == "generation"]
        for s_, (sk, tasks) in sig.items():
            if s_ not in dra:
                rb.append(f"op {i}: {s_} was signalled but its drain was not logged")
                continue
            dk, done_, stalled_ = dra[s_]
            if sorted(done_ + stalled_) != sorted(tasks) or dk < sk:
                rb.append(f"op {i}: {s_}'s drain {done_}+{stalled_} does not match its signal {tasks}")
            for rk_, _ in pos.get(("release", s_), []):
                if rk_ < dk or stalled_:
                    rb.append(f"op {i}: {s_}'s state was released before its drain ended")
            if exp["actions"].get(s_) == "old-reader":
                if gen_at and gen_at[0] < dk:
                    rb.append(f"op {i}: the new generation was installed before {s_}'s old readers drained")
                for cid in exp["cancelled"].get(s_, []):
                    ks = [k for k, _ in pos.get(("completed", cid), [])]
                    if ks and gen_at and ks[0] > gen_at[0]:
                        rb.append(f"op {i}: old reader {cid} completed after the new generation was installed")
        for s in exp["actions"]:
            if exp["actions"][s] not in ("old-reader",) and exp["cancelled"].get(s) and s not in sig and op["op"] != "caps":
                rb.append(f"op {i}: {s}'s invocations were cancelled without a signal")
            if exp["actions"][s] == "old-reader":
                continue
            rels = pos.get(("release", s), [])
            if len(rels) != 1:
                rb.append(f"op {i}: {len(rels)} releases of {s}'s old state")
                continue
            rk, rg = rels[0]
            if rg != oracle_cur_before(outs, i, ops):
                rb.append(f"op {i}: {s}'s old state was released on generation {rg} (expected {oracle_cur_before(outs, i, ops)})")
            # the barrier: every cancelled invocation has completed before the state is released
            for cid in exp["cancelled"].get(s, []):
                inv = next((x for x in oracle.invs if x.id == cid), None)
                if inv is not None and inv.outcome == "cancelled" and cid in exp["completed"]:
                    ks = [k for k, _ in pos.get(("completed", cid), [])]
                    if not ks or ks[0] > rk:
                        rb.append(f"op {i}: {cid} completed after {s}'s old state was released (or not at all)")
            for k, _ in pos.get(("started", s), []):
                if k < rk:
                    n4.append(f"op {i}: {s} was rebuilt before its old state was released")
            want_r = sum(1 for p in exp["session_prints"] if p.startswith("receipt closed"))
            got_r = [k for k, e in enumerate(evs) if e["event"] == "print" and e["id"] == "session" and e["text"].startswith("receipt closed")]
            if s == "shop" and (len(got_r) != want_r or any(k < rk for k in got_r)):
                rb.append(f"op {i}: {len(got_r)} cleanups of the unawaited future's receipt (expected {want_r}), after the release")
            audits = pos.get(("audit", s), [])
            if s in ("shop", "meter"):
                if len(audits) != 1:
                    rb.append(f"op {i}: {len(audits)} cleanups of {s}'s old state")
                    continue
                ak, ag = audits[0]
                rel = oracle_cur_before(outs, i, ops)
                if ak < rk:
                    rb.append(f"op {i}: {s}'s old state's cleanup ran before its release")
                if ag != rel:
                    rb.append(f"op {i}: {s}'s old state was released on generation {ag} (expected {rel})")
                for cid in exp["cancelled"].get(s, []):
                    for k, _ in pos.get(("print", cid), []):
                        if k > ak:
                            rb.append(f"op {i}: {cid} printed after {s}'s old state was released")
                for k, _ in pos.get(("started", s), []):
                    if k < ak:
                        n4.append(f"op {i}: {s} was rebuilt before its old state's cleanup ran")
            st = pos.get(("started", s), [])
            if exp["actions"][s] == "entry":
                stopped.add(s)
                if st:
                    rc.append(f"op {i}: stopped scope {s} was rebuilt")
            else:
                want = [g for sc, g in exp["started"] if sc == s]
                if [g for _, g in st] != want:
                    rc.append(f"op {i}: {s} rebuilt on {[g for _, g in st]} (expected {want})")
        # explanations: every restart, stop and old-reader cancellation, against the generations' sources
        rel_ch = [x for x in evs if x["event"] == "reload"]
        incompatible = {c["ident"] for x in rel_ch for c in x["changes"] if not c["compatible"]}
        if op["op"] == "reload":
            old_src, new_src = version_text(ops, i, before=True), version_text(ops, i, before=False)
            fo, fno = source_index(old_src)
            fn_, fnn = source_index(new_src)
        for e in evs:
            if e["event"] == "stopped" and e["rule"] == "entry" and FACTORIES[e["scope"]] not in e.get("message", ""):
                xx.append(f"op {i}: {e['scope']} stopped without naming its factory: {e.get('message')!r}")
            if e["event"] not in ("restart", "old-reader") or e.get("rule") in ("requested", "generation-cap", "memory-cap", "version-conflict"):
                continue
            path = e.get("path", [])
            if len(path) < 2:
                xx.append(f"op {i}: {e['scope']} path {path} has fewer than two steps")
                continue
            root_ok = {"state": path[0] == f"state: {STATE_TYPES[e['scope']]}", "old-reader": path[0] == f"state: {STATE_TYPES[e['scope']]}",
                       "dependency": bool(re.match(r"(factory|handler) main::function \w+$", path[0]))}.get(e["rule"], True)
            if e["rule"] == "di":
                root_ok = path[0] == f"state: {STATE_TYPES[e['scope']]}" or bool(re.match(r"(factory|handler) main::function \w+$", path[0]))
            if not root_ok:
                xx.append(f"op {i}: {e['scope']}'s {e['rule']} path starts {path[0]!r}")
            if path[0].startswith("factory ") and not path[0].endswith(FACTORIES[e["scope"]]):
                xx.append(f"op {i}: {e['scope']}'s path starts at another scope's factory: {path[0]!r}")
            if path[0].startswith("handler "):
                h = path[0].rsplit(" ", 1)[-1]
                if not re.search(rf"func {h}\(\w+: (owned )?{re.escape(STATE_TYPES[e['scope']])}", old_src + new_src):
                    xx.append(f"op {i}: {path[0]!r} is not a handler of {e['scope']}")
            if e["rule"] == "old-reader" and not any("(interior mutability)" in p for p in path):
                xx.append(f"op {i}: an old-reader path reaches no interior mutability: {path}")
            if e["rule"] == "di" and not any("DI context" in p or "(di" in p for p in path + [e.get("change", "")]):
                xx.append(f"op {i}: a di restart names no context: {path}")
            forms = [r"\w+(\.\w+){1,2}: .+", r"(value of|future of) main::(function|method|cleanup) .+", r"capture \d+: .+", r"argument: .+", r"failure main::\w+",
                     r"dynamic \w+ → \w+", r"Mutex\[.+\] \(interior mutability\)", r"calls main::.+", r"may dispatch to main::.+", r"may call main::.+ through a value",
                     r"type main::\w+", r"method main::.+", r"cleanup main::.+", r"reads main::\w+"]
            body_steps = path[1:-1]
            if path and path[-1].startswith("generated by the DI context"):
                body_steps = path[1:-2]
            for step in body_steps:
                if not any(re.fullmatch(f, step) for f in forms):
                    xx.append(f"op {i}: step {step!r} is not a form an explanation has")
            names_all = set(fo) | set(fno) | set(fn_) | set(fnn) | set(re.findall(r"^const (\w+)", old_src + "\n" + new_src, re.M)) | set(re.findall(r"^(?:export )?(?:class|enum|error|interface|mixin) (\w+)", old_src + "\n" + new_src, re.M))
            short = re.sub(r"^main::(function |method |cleanup )?", "", e["ident"]).split("/")[0]
            contexts = re.findall(r"^context (\w+)", old_src + "\n" + new_src, re.M)
            if not any(x in names_all or any(x.startswith(c) for c in contexts) for x in (short, short.split(".")[0], short.split(".")[-1])):
                xx.append(f"op {i}: {e['scope']}'s explanation names {e['ident']!r}, which neither version declares")
            terminal = path[-2] if path[-1].startswith("generated by the DI context") else path[-1]
            if e["rule"] != "old-reader" and not (e["ident"] in terminal or short in terminal or short.split(".")[-1] in terminal):
                xx.append(f"op {i}: {e['scope']}'s explanation ends {terminal!r}, not at {e['ident']}")
            # a dependency path's steps must be edges of the source: a call in the previous function's body, a constant
            # it reads, a type it mentions, a method it may dispatch to
            if e["rule"] in ("dependency", "di") and re.match(r"(factory|handler) ", path[0]):
                bodies = function_bodies(old_src)
                bodies.update({k: v for k, v in function_bodies(new_src).items() if k not in bodies})
                fn = path[0].rsplit(" ", 1)[-1]
                for step in body_steps:
                    m = re.fullmatch(r"(calls|may call) main::(?:function|method|cleanup) ([\w.]+)(?: through a value)?", step)
                    here = bodies.get(fn, "")
                    if m and m.group(1) == "calls" and fn in bodies and not re.search(rf"\b{re.escape(m.group(2).split('.')[-1])}\b", here):
                        xx.append(f"op {i}: {fn} does not call {m.group(2)} ({step!r})")
                    m2 = re.fullmatch(r"reads main::(\w+)", step)
                    if m2 and fn in bodies and not re.search(rf"\b{m2.group(1)}\b", here):
                        xx.append(f"op {i}: {fn} does not read {m2.group(1)} ({step!r})")
                    m3 = re.fullmatch(r"(?:calls|may call|may dispatch to|method|cleanup) main::(?:function|method|cleanup) ([\w.]+)(?: through a value)?", step)
                    if m3:
                        fn = m3.group(1).split(".")[-1]
                    m4 = re.fullmatch(r"reads main::(\w+)", step)
                    if m4:
                        fn = m4.group(1)
            prev = path[0]
            for step in path[1:-1]:
                m = re.match(r"(\w+)((?:\.\w+){1,2}): (.*)$", step)
                if m:
                    owner, field = m.group(1), m.group(1) + m.group(2)
                    if field not in fo and field not in fn_:
                        xx.append(f"op {i}: step {step!r} is not a field of either version")
                    if not re.search(rf"\b{owner}\b", prev):
                        xx.append(f"op {i}: step {step!r} does not follow from {prev!r}")
                m = re.match(r"(?:calls|handler|factory|may dispatch to|may call|value of|future of|method|cleanup) main::(?:function|method|cleanup) ([\w.]+)", step)
                if m and not any(x in fno or x in fnn for x in (m.group(1), m.group(1).split(".")[-1])):
                    xx.append(f"op {i}: step {step!r} names no function of either version")
                if step.startswith(("value of", "future of")) and not re.search(r"fn\(|Future\[|Task\[|dynamic fn", prev):
                    xx.append(f"op {i}: step {step!r} does not follow from {prev!r}")
                if step.startswith("capture ") and not prev.startswith(("value of", "future of")) and not prev.startswith("capture"):
                    xx.append(f"op {i}: step {step!r} does not follow from {prev!r}")
                if step.startswith("Mutex[") and "Mutex" not in prev:
                    xx.append(f"op {i}: step {step!r} does not follow from {prev!r}")
                prev = step
            if e["rule"] == "dependency" and e["ident"] not in incompatible:
                n2.append(f"op {i}: {e['scope']} restarted for {e['ident']}, which has no incompatible change")
            if e["rule"] in ("state", "di", "old-reader"):
                short = {x.split("::")[-1].split(" ")[-1] for x in incompatible} | {x.split("::")[-1] for x in incompatible}
                tied = e["ident"] in incompatible or any(re.search(rf"\b{re.escape(n)}\b", " ".join(path)) for n in short if n)
                if e["rule"] != "old-reader" and not tied:
                    n2.append(f"op {i}: {e['scope']}'s {e['rule']} restart for {e['ident']} ties to no incompatible change")
                if not re.search(r"missing|changed|lost|no longer|does not|implemented by|\(di: |DI context", path[-1]):
                    n2.append(f"op {i}: {e['scope']}'s {e['rule']} explanation ends {path[-1]!r}")
        for x in evs:
            # evictions under the caps are retention restarts, not the reload's: they may follow a compatible reload
            if x["event"] == "reload" and x["compatible"] and any(v not in ("old-reader", "generation-cap", "memory-cap") for v in raw_acts.values()):
                n3.append(f"op {i}: a compatible reload restarted or stopped {raw_acts}")
        if op["op"] == "call" and op["scope"] in stopped:
            if not any(e["event"] == "error" for e in evs) or any(e["event"] == "invocation" for e in evs):
                n5.append(f"op {i}: a call to stopped scope {op['scope']} ran")
        # retention
        rec = sorted(e["gen"] for e in evs if e["event"] == "reclaimed")
        if rec != sorted(exp["reclaimed"]):
            g1.append(f"op {i}: reclaimed {rec} (expected {sorted(exp['reclaimed'])})")
        for e in evs:
            if e["event"] == "snapshot":
                es = exp.get("snapshot")
                if es and (e["live"] != es["live"] or e["pinned"] != es["pinned"] or e["current"] != es["current"]):
                    g1.append(f"op {i}: live {e['live']} pinned {e['pinned']} (expected {es['live']} {es['pinned']})")
                if e["objects"] != len(e["live"]):
                    g2.append(f"op {i}: {e['objects']} generation objects for {len(e['live'])} live")
                if not set(e.get("const_gens", [])) <= set(e["live"]):
                    g2.append(f"op {i}: constants are cached for generations {e['const_gens']}, live {e['live']}")
                if caps:
                    gcap, bcap = caps_at(ops, i, sizes)
                    if gcap is not None and len(e["live"]) > gcap:
                        g3.append(f"op {i}: {len(e['live'])} live generations over the cap {gcap}")
                    if bcap is not None and e["old_bytes"] > bcap:
                        g3.append(f"op {i}: {e['old_bytes']} old bytes over the cap {bcap}")
    # G3: the sizes the session counts are the sizes measured apart from it
    if ir_sizes:
        for e in events:
            if e["event"] == "generation" and ir_sizes.get(e["source"]) != e["bytes"]:
                g3.append(f"generation {e['gen']} ({e['source']}) counted {e['bytes']} bytes, measured {ir_sizes.get(e['source'])}")
    res.add("R-a", name, not ra, "; ".join(ra[:5]))
    res.add("R-b", name, not rb, "; ".join(rb[:5]))
    res.add("R-c", name, not rc, "; ".join(rc[:5]))
    res.add("X", name, not xx, "; ".join(xx[:5]))
    res.add("G1", name, not g1, "; ".join(g1[:5]))
    res.add("G2", name, not g2, "; ".join(g2[:5]))
    res.add("N2", name, not n2, "; ".join(n2[:5]))
    res.add("N3", name, not n3, "; ".join(n3[:5]))
    res.add("N4", name, not n4, "; ".join(n4[:5]))
    if any(op["op"] == "call" and op["scope"] in stopped for op in ops):
        res.add("N5", name, not n5, "; ".join(n5[:5]))
    if caps:
        res.add("G3", name, not g3, "; ".join(g3[:5]))
    res.add("K1", name, not any(e["event"] == "conflict" or (e["event"] == "completed" and e["outcome"] == "conflict") for e in events), "a conflict was logged")
    # G4: at finish one generation is live and no task thread runs
    fin = [e for e in actual[-1]["events"] if e["event"] == "snapshot"] if actual and actual[-1]["op"] == "finish" else []
    threads = metrics[-1]["threads"] if metrics else None
    ok = bool(fin) and fin[-1]["live"] == [fin[-1]["current"]] and threads == 0
    res.add("G4", name, ok, f"final snapshot {fin[-1] if fin else None}, threads {threads}")
    return oracle, outs


def oracle_cur_before(outs, i, ops):
    """The current generation just before operation i, by counting installed generations in the ops before it."""
    g = 0
    for op, out in zip(ops[:i], outs[:i]):
        if op["op"] == "open":
            g = 1
        elif op["op"] == "reload" and not out["rejected"]:
            g += 1
    return g


def version_text(ops, i, before):
    vs = [op["_version"] for op in ops[: i + (0 if before else 1)] if "_version" in op and not any(CAT[e].get("rejected") for e in op["_version"][1])]
    v, edits = vs[-1]
    return generate.text(v, edits)


def caps_at(ops, i, sizes):
    g = b = None
    for op in ops[: i + 1]:
        if op["op"] == "caps":
            g = op.get("generations", g)
            if "_bytes" in op:
                b = int(op["_bytes"] * CAP_UNIT[0])
    return g, b


# the byte cap's unit: a generation's size measured apart from the session (`fortisc dev --ir-size`)
CAP_UNIT = [0]


def check_sequence(res, name, ops, events):
    """C on a generated sequence: each reload is compatible exactly when every edit it turns on reports only `body`
    and `added` changes and every edit it turns off reports only `body` and `removed` changes (turning an edit off
    reverses its changes: an addition becomes a removal, a removal an addition)."""
    actual = split(events)
    prev = None
    problems = []
    for i, (op, act) in enumerate(zip(ops, actual)):
        if "_version" not in op:
            continue
        v, edits = op["_version"]
        if any(CAT[e].get("rejected") for e in edits):
            continue
        if prev is not None and op["op"] == "reload":
            on, off = set(edits) - set(prev[1]), set(prev[1]) - set(edits)
            kinds_on = {k for e in on for _, k in CAT[e]["changes"]}
            kinds_off = {k for e in off for _, k in CAT[e]["changes"]}
            # turning an edit off reverses its changes: an addition becomes a removal, a removal an addition
            want = kinds_on <= {"body", "added"} and kinds_off <= {"body", "removed"}
            rel = [e for e in act["events"] if e["event"] == "reload"]
            if not rel or rel[0]["compatible"] != want:
                problems.append(f"op {i}: turning on {sorted(on)} and off {sorted(off)} gave compatible={rel[0]['compatible'] if rel else None} (expected {want})")
        prev = (v, edits)
    res.add("C", f"{name}.sequence", not problems, "; ".join(problems[:5]))


def check_classification(res, name, events, expected, compatible=None):
    rel = [e for e in events if e["event"] == "reload"]
    if not rel:
        res.add("C", name, False, "no reload event")
        return
    got = [(c["ident"], c["kind"], c["compatible"]) for c in rel[0]["changes"]]
    problems = []
    for g in got:
        if not any(fnmatch.fnmatchcase(g[0], p) and g[1] == k for p, k in expected):
            problems.append(f"unexpected {g[:2]}")
        if g[2] != (g[1] in ("body", "added")):
            problems.append(f"{g[:2]} marked compatible={g[2]}")
    used = []
    for p, k in expected:
        hits = [x for x in got if fnmatch.fnmatchcase(x[0], p) and x[1] == k and x not in used]
        if not hits:
            problems.append(f"missing {[p, k]}")
        else:
            used.append(hits[0])
    want_c = all(k in ("body", "added") for _, k in expected) if compatible is None else compatible
    if rel[0]["compatible"] != want_c:
        problems.append(f"reload compatible={rel[0]['compatible']} (expected {want_c})")
    res.add("C", name, not problems, "; ".join(problems[:6]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", required=True)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--soak", type=int, default=1000)
    ap.add_argument("--fortisc", default=os.path.join(ROOT, "compiler", "target", "release", "fortisc"))
    ap.add_argument("--fault-fortisc", default=None, help="a fortisc built with --features reload-faults, for K2")
    ap.add_argument("--fault", default=None)
    ap.add_argument("--rss-bound-kb", type=int, default=8192)
    ap.add_argument("--work", default=None, help="where the sessions' logs go (default build/reload, or build/reload-<fault>)")
    a = ap.parse_args()
    t0 = time.time()
    res = Results()
    a.work = a.work or os.path.join(ROOT, "build", "reload" + (f"-{a.fault}" if a.fault else ""))
    runner = Runner(a.fortisc, a.work, a.fault)
    scs = scenarios(a.seed, a.soak)
    exceptions = []
    size = None
    for name, ops in scs.items():
        try:
            ir_sizes = None
            if name == "caps-bytes":
                # sizes measured apart from the session: the cap's unit, and what G3 compares the session's counts with
                ir_sizes = runner.sizes("caps-bytes-sizes", [op["_version"] for op in ops if "_version" in op])
                CAP_UNIT[0] = ir_sizes[generate.name(1, [])]
            events, metrics, text, real = runner.run(name, ops, size)
            caps = name.startswith("caps") or name == "detached-caps"
            oracle, outs = check_scenario(res, name, ops, events, metrics, runner, caps=caps, coarse=name == "soak", ir_sizes=ir_sizes)
            # C: a single edit from the base, and the version bump
            if name.startswith("edit-"):
                e = CAT[name[5:]]
                if e.get("rejected"):
                    rej = [x for x in events if x["event"] == "rejected"]
                    res.add("C", name, bool(rej) and not any(x["event"] == "reload" for x in events), "the source was not rejected")
                else:
                    check_classification(res, name, events, e["changes"])
            if name == "bump":
                check_classification(res, name, events, [[f, "body"] for f in BUMP])
            if name in ("soak", "three-gen"):
                check_sequence(res, name, ops, events)
            # D: the same script again gives the same log
            _, _, text2, _ = runner.run(name, ops, size, tag="-again")
            res.add("D", name, text == text2, "the event logs differ")
            if name == "soak":
                check_soak(res, events, metrics, oracle, a.rss_bound_kb)
        except SessionCrash as ex:  # the session crashed: a failed check of every family, the run stays complete
            for fam in FAMILIES:
                res.add(fam, f"{name}.crash", False, str(ex)[:1500])
        except Exception as ex:  # the harness failed: a failed check of every family, and the run is incomplete
            exceptions.append(f"{name}: {ex}")
            for fam in FAMILIES:
                res.add(fam, f"{name}.exception", False, f"{ex}\n{traceback.format_exc()[-1500:]}")
    # J: a rejected reload changes nothing
    try:
        ops = [o_open(1)] + starts() + [o_call("shop", "checkout", "j1", cents=10, pause=50), o_run(10), o_reload(1, ["type-error"]),
                                       o_call("shop", "checkout", "j2", cents=10, pause=5), o_run(100), {"op": "finish"}]
        without = [op for op in ops if op.get("_version") != (1, ["type-error"])]
        e1, _, _, _ = runner.run("rejected", ops)
        e2, _, _, _ = runner.run("rejected-without", without)
        strip = []
        drop = False
        for e in e1:
            if e["event"] == "op":
                drop = e["op"] == "reload"
                if drop:
                    continue
            if drop:
                continue
            strip.append(e)
        norm = lambda es: [{k: v for k, v in e.items() if k != "index"} for e in es]
        res.add("J", "rejected", norm(strip) == norm(e2) and any(e["event"] == "rejected" for e in e1), "the log with the rejected reload differs from the log without it")
    except SessionCrash as ex:
        res.add("J", "rejected.crash", False, str(ex)[:1500])
    except Exception as ex:
        exceptions.append(f"rejected: {ex}")
        res.add("J", "rejected.exception", False, str(ex))
    # on the fault build: K2, a stale value meets its conflict (async and synchronous) and its scope restarts; and
    # G3's stall, a drain that cannot finish without time
    # each fault-build scenario has its own switch, combined with the apparatus check's fault under test
    with_fault = lambda own: ",".join(x for x in (own, a.fault) if x)
    if a.fault_fortisc:
        try:
            kr = Runner(a.fault_fortisc, a.work + "-k2", with_fault("skip-shop"))
            ops = [o_open(1)] + starts() + [o_reload(1, ["config-layout"]), o_call("shop", "checkout", "z1", cents=10, pause=0), o_call("shop", "checkout", "z2", cents=10, pause=0),
                                           o_reload(1, []), o_call("shop", "quote", "z3", cents=1), o_call("shop", "quote", "z4", cents=1), {"op": "finish"}]
            ev, _, _, _ = kr.run("conflict", ops)
            conf = [e["id"] for e in ev if e["event"] == "conflict"]
            comp = {e["id"]: e for e in ev if e["event"] == "completed"}
            rs = [e for e in ev if e["event"] == "restart" and e["rule"] == "version-conflict" and e["scope"] == "shop"]
            ran = [x for x in kr.frames if x["id"] in ("z1", "z3") and "λ" in x["ident"]]
            ok = conf == ["z1", "z3"] and comp.get("z1", {}).get("outcome") == "conflict" and comp.get("z3", {}).get("outcome") == "conflict" and len(rs) == 2 and comp.get("z2", {}).get("outcome") == "ok" and comp.get("z4", {}).get("outcome") == "ok" and not ran
            res.add("K2", "conflict", ok, f"conflicts {conf}, z1 {comp.get('z1')}, z3 {comp.get('z3')}, restarts {len(rs)}, z2 {comp.get('z2', {}).get('outcome')}, z4 {comp.get('z4', {}).get('outcome')}, stale frames run {ran}")
        except SessionCrash as ex:
            res.add("K2", "conflict.crash", False, str(ex)[:1500])
        except Exception as ex:
            exceptions.append(f"conflict: {ex}")
            res.add("K2", "conflict.exception", False, str(ex))
        # the stall under an eviction (a version bump) and under an ordinary restart (an incompatible edit)
        for sname, target in (("stall", (2, [])), ("stall-restart", (1, ["config-layout"]))):
            try:
                sr = Runner(a.fault_fortisc, a.work + "-stall", with_fault("stall-nap"))
                ops = [o_open(1)] + starts() + [{"op": "caps", "generations": 1}, o_call("shop", "fanout", "st1", pause=4), o_run(5), o_reload(*target), {"op": "snapshot"},
                                               o_call("shop", "balance", "st2"), o_run(1000), {"op": "snapshot"}, {"op": "finish"}]
                ev, _, _, _ = sr.run(sname, ops)
                sp = split(ev)
                # ops: 7 the reload, 8 a snapshot, 9 a call to the stopped scope, 10 time enough for the stalled task
                rej = [e for e in sp[7]["events"] if e["event"] == "rejected" and e.get("reason") == "retention"]
                stop = [e for e in sp[7]["events"] if e["event"] == "stopped" and e["rule"] == "stall" and e["scope"] == "shop"]
                early = [e for x in sp[7:10] for e in x["events"] if e["event"] == "release" and e["scope"] == "shop"]
                later = [e for x in sp[10:] for e in x["events"] if e["event"] in ("release", "completed") and e.get("scope", e.get("id")) in ("shop", "st1")]
                snap1 = [e for e in sp[8]["events"] if e["event"] == "snapshot"]
                refused = any(e["event"] == "error" for e in sp[9]["events"]) and not any(e["event"] == "invocation" for e in sp[9]["events"])
                order = [e["event"] for e in later]
                ok = bool(rej) and bool(stop) and not early and snap1 and snap1[0]["current"] == 1 and refused and order[:2] == ["completed", "release"]
                res.add("G3", sname, ok, f"rejected {rej}, stopped {stop}, released early {early}, snapshot {snap1}, refused {refused}, later {order}")
            except SessionCrash as ex:
                res.add("G3", f"{sname}.crash", False, str(ex)[:1500])
            except Exception as ex:
                exceptions.append(f"{sname}: {ex}")
                res.add("G3", f"{sname}.exception", False, str(ex))
        # a stalled scope's kept state counts against the caps on later reloads
        try:
            sr = Runner(a.fault_fortisc, a.work + "-stall", with_fault("stall-nap"))
            ops = [o_open(1)] + starts() + [{"op": "caps", "generations": 3}, o_call("shop", "fanout", "sh1", pause=4), o_run(5), o_reload(2), o_reload(2, ["config-layout"]),
                                           {"op": "snapshot"}, o_call("meter", "hit", "sh2", n=1, pause=2000), o_reload(3, ["config-layout"]), {"op": "snapshot"},
                                           o_run(3000), {"op": "snapshot"}, {"op": "finish"}]
            ev, _, _, _ = sr.run("stall-held", ops)
            snaps = [e for e in ev if e["event"] == "snapshot"]
            over = [e["live"] for e in snaps if len(e["live"]) > 3]
            ok = bool(snaps) and not over and snaps[-1]["live"] == [snaps[-1]["current"]]
            res.add("G3", "stall-held", ok, f"snapshots over the cap {over}; last {snaps[-1] if snaps else None}")
        except SessionCrash as ex:
            res.add("G3", "stall-held.crash", False, str(ex)[:1500])
        except Exception as ex:
            exceptions.append(f"stall-held: {ex}")
            res.add("G3", "stall-held.exception", False, str(ex))
        # old readers that cannot drain keep the reload out; once they finish it installs
        try:
            sr = Runner(a.fault_fortisc, a.work + "-stall", with_fault("stall-nap"))
            ops = [o_open(1)] + starts() + [o_call("shop", "fanout", "sr1", pause=4), o_run(5), o_reload(1, ["hooks-backflow"]), o_call("shop", "balance", "sr2"),
                                           o_run(1000), o_reload(1, ["hooks-backflow"]), {"op": "finish"}]
            ev, _, _, _ = sr.run("stall-reader", ops)
            sp = split(ev)
            first = [e for e in sp[6]["events"] if e["event"] in ("rejected", "generation")]
            later_gen = [e for e in sp[9]["events"] if e["event"] == "generation"]
            comp = {e["id"]: e for e in ev if e["event"] == "completed"}
            ok = [e["event"] for e in first] == ["rejected"] and first[0].get("reason") == "old-readers" and comp.get("sr2", {}).get("outcome") == "ok" and comp.get("sr2", {}).get("gen") == 1 and len(later_gen) == 1
            res.add("G3", "stall-reader", ok, f"first reload {first}, sr2 {comp.get('sr2')}, second reload installed {later_gen}")
        except SessionCrash as ex:
            res.add("G3", "stall-reader.crash", False, str(ex)[:1500])
        except Exception as ex:
            exceptions.append(f"stall-reader: {ex}")
            res.add("G3", "stall-reader.exception", False, str(ex))
    present = {c["family"] for c in res.checks}
    for fam in FAMILIES:
        if fam not in present and not (fam == "K2" and not a.fault_fortisc):
            res.add(fam, "incomplete", False, "no check of this family ran")
    complete = not exceptions and all(not c["name"].endswith(".incomplete") for c in res.checks)
    passed = sum(c["pass"] for c in res.checks)
    out = {"schema": "fortis.reload-check/v1", "complete": complete, "seed": a.seed, "soak": a.soak, "fault": a.fault, "passed": passed, "total": len(res.checks), "seconds": round(time.time() - t0, 1), "exceptions": exceptions, "checks": res.checks}
    with open(a.json, "w", encoding="utf-8") as f:
        json.dump(out, f, indent=1, ensure_ascii=False)
        f.write("\n")
    for c in res.checks:
        if not c["pass"]:
            print(f"FAIL {c['name']}: {c['detail'][:400]}")
    print(f"reload check: {passed}/{len(res.checks)} passed, complete={complete}, {out['seconds']} s")
    return 0 if passed == len(res.checks) and complete else 1


def check_soak(res, events, metrics, oracle, bound_kb):
    snaps = [m for m in metrics]
    # the metrics file has one line per snapshot: the soak's snapshot after reload r is the r-th after the caps
    rss = [m["rss_kb"] for m in snaps]
    roots = [m["roots"] for m in snaps]
    records = [m["task_records"] for m in snaps]
    reloads = len(rss) - 1
    if reloads < 1000:
        i100, iend = max(0, reloads // 10 - 1), reloads - 1
    else:
        i100, iend = 99, 999
    growth = rss[iend] - rss[i100]
    per_root = 2.0  # KiB per root invocation started: its task records and the session's record of it (design 10)
    allowance = bound_kb + per_root * (roots[iend] - roots[i100])
    res.add("S", "rss", growth <= allowance, f"resident memory grew {growth} KiB from reload {i100 + 1} to {iend + 1} (allowance {allowance:.0f} KiB; task records {records[i100]} → {records[iend]})")
    # the measurements, kept whether or not the check passed
    res.checks[-1]["measured"] = {"rss_kb_at": [rss[i100], rss[iend]], "growth_kb": growth, "allowance_kb": round(allowance), "roots": [roots[i100], roots[iend]], "task_records": [records[i100], records[iend]]}
    final = [e for e in events if e["event"] == "snapshot"][-1]
    res.add("S", "final", final["live"] == [final["current"]], f"final snapshot {final}")
    live_max = max(len(e["live"]) for e in events if e["event"] == "snapshot")
    res.add("S", "cap", live_max <= 3, f"{live_max} live generations at most")
    res.add("S", "metrics", len(rss) >= 2, "too few samples")


if __name__ == "__main__":
    sys.exit(main())
