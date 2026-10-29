#!/usr/bin/env python3
"""SP-09-telemetry's harness: the measures of SP-09-telemetry's design, section 6 (P1-P4, D1-D4, C1, U1,
E1, V1, A1, B1, L1, S1), over the workload conformance/telemetry/orders.ft (and its version-1 build) on the
interpreter, the VM and native, and the plans in conformance/telemetry/plans.

The oracle is written from the design and the workload's own formulas. The expected canonical records are built from
the formulas, and every metric expectation is computed from those formula records, looked up by the identity of
each delivered line, never from a record the adapter or the pipeline produced. Recovery and replay are also
compared with a clean run, as separate assertions of equivalence. The pipeline is only driven, through its API, and
through its command line for the SIGKILL runs.

Each section of the run is isolated: an exception inside one fails that section's family (`<FAMILY>.exception`)
and the run goes on. Everything the harness and the programs write, its own output included, goes under one
directory that P1 scans last.

  python3 conformance/tools/telemetry_check.py [--json OUT] [--seed N] [--permutations N] [--crash-stride N] [--kills N]
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import random
import re
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import traceback
from decimal import Decimal

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, ROOT)

from telemetry import plan as planmod  # noqa: E402  (the system under test, driven only)
from telemetry import yamlite  # noqa: E402
from telemetry.disk import Crash, RealDisk, SimDisk  # noqa: E402
from telemetry.pipeline import ArchiveRefused, Pipeline  # noqa: E402
from telemetry.registry import Registry  # noqa: E402

FORTIS = os.path.join(ROOT, "bin", "fortis")
FORTISC = os.path.join(ROOT, "compiler", "target", "release", "fortisc")
WORKLOAD = os.path.join(ROOT, "conformance", "telemetry", "orders.ft")
WORKLOAD_V1 = os.path.join(ROOT, "conformance", "telemetry", "orders_v1.ft")
PLANS = os.path.join(ROOT, "conformance", "telemetry", "plans")
BACKENDS = ("interp", "vm", "native")
STRING_TYPE = "String<scalars:0..64>"           # the declared type as the policy spells it (design 3.1.3)
CANARY_ALPHABET = "ghjkmnpqrstvwxyz"             # no hex digit, so no canary piece can occur inside a digest
ACK_DEADLINE = 30                                # seconds a live pipeline may take to acknowledge one batch

RESULTS = []


def check(name: str, ok: bool, detail=None):
    RESULTS.append({"check": name, "pass": bool(ok), "detail": detail})
    print(f"{'PASS' if ok else 'FAIL'} {name}" + ("" if ok or detail is None else f": {json.dumps(detail, default=str)[:600]}"), flush=True)
    return ok


class Tee:
    """The harness's own output, to the terminal and to a file the egress scan reads."""

    def __init__(self, stream, path):
        self.stream, self.f = stream, open(path, "a", encoding="utf-8")

    def write(self, s):
        self.stream.write(s)
        self.f.write(s)
        self.f.flush()

    def flush(self):
        self.stream.flush()


# ---------------------------------------------------------------- the workload's formulas (the oracle's ground truth)

REGIONS, TIERS, PROVIDERS = ("UK", "IE", "FR", "DE"), ("Basic", "Business", "Enterprise"), ("Stripe", "Adyen", "Braintree")


def region(n): return REGIONS[(n * 7) % 4]
def tier(n): return TIERS[(n * 5) % 3]
def provider(n): return PROVIDERS[0 if n % 9 < 3 else 1 if n % 9 < 6 else 2]
def amount(n): return (n % 17) * 100 + 99
def end_tier(s): return tier(s + 1 if s % 3 == 0 else s + 2 if s % 6 == 1 else s)


def keyed(key_hex: str, key_id: str, type_: str, text: str) -> str:
    msg = b"fortis.telemetry.hash.v1\x00" + type_.encode() + b"\x00" + text.encode()
    return f"h1:{key_id}:" + hmac.new(bytes.fromhex(key_hex), msg, hashlib.sha256).hexdigest()[:32]


def expected_channel(version: int, cores: dict, key_hex: str, key_id: str, producer: str, hash_session=False):
    """The canonical permitted records the main plan's policy must produce for the workload (design 3.1)."""
    out, seq = [], [0]

    def rec(schema, ver, fields, function=None, outcome=None):
        seq[0] += 1
        r = {"fields": fields, "id": {"producer": producer, "seq": seq[0]}, "kind": "observation-frame" if function else "observation-event",
             "schema": schema, "truncated": [], "version": ver, "withheld": 0}
        if function:
            r["function"], r["outcome"] = function, outcome
        out.append(json.dumps(r, sort_keys=True))

    def session(s, t, phase):
        f = {"phase": str(phase), "tier": t}
        if s is not None:
            f["session"] = keyed(key_hex, key_id, "Int", str(s)) if hash_session else str(s)
        rec("Session", 1, f)

    for n in range(240):
        email = f"{cores['email']}u{n % 40}@example.org"[:64]
        f = {"amount": str(amount(n)), "email": keyed(key_hex, key_id, STRING_TYPE, email), "region": region(n), "tier": tier(n)}
        if version == 2:
            f.update(provider=provider(n), note="[redacted]", sku=f"sku{n % 5}")      # card is dropped
            if n % 4 == 1:
                f["discount"] = str(n * 3)
        rec("OrderProcessing", version, f, "processOrder", "success")
        if n % 10 == 0:
            rec("Audit", 1, {"order": str(n)})
    for s in range(60):
        session(s, tier(s), 1)
        session(s, end_tier(s), 2)
    for s, t, ph in ((60, "Business", 1), (60, "Business", 1), (60, "Business", 2), (61, "Business", 2),
                     (62, "Enterprise", 1), (62, "Enterprise", 2), (62, "Enterprise", 1), (62, "Enterprise", 2), (None, "Business", 1)):
        session(s, t, ph)
    return out


def formula_map(lines):
    return {(r["id"]["producer"], r["id"]["seq"]): r for r in map(json.loads, lines)}


def by_identity(lines, formula):
    """The formula record of each delivered line, by the line's identity only (the line's own content is not used)."""
    out = []
    for ln in lines:
        try:
            i = json.loads(ln)["id"]
            out.append(formula[(i["producer"], i["seq"])])
        except (ValueError, KeyError, TypeError):
            continue
    return out


# The plans' meaning, written by hand from each YAML file (never read from it).
def is_v2(r): return r["schema"] == "OrderProcessing" and r["version"] == 2
def route(r): return is_v2(r) and r["fields"]["region"] in ("UK", "IE", "FR") and "email" in r["fields"]
def premium(r): return r["schema"] == "OrderProcessing" and r["version"] in (1, 2) and r["fields"]["tier"] != "Basic"


def lifecycle_spec(metrics, tiers, limit=1000):
    return {"lifecycle": True, "metrics": metrics, "start_member": lambda r: r["fields"]["tier"] in tiers, "dims": ("tier",), "limit": limit}


ROUTE = {"member": route, "dims": ("region", "tier", "provider"), "limit": 20,
         "metrics": {"orders.count": ("counter",), "orders.amount": ("histogram", "amount", (500, 1000, 1500))}}
PREMIUM = {"member": premium, "dims": ("region", "tier"), "limit": 1000, "metrics": {"orders.premium": ("counter",)}}
SESSIONS = lifecycle_spec(("sessions.open",), ("Business", "Enterprise"))
ORACLE = {
    "main": {"orders_by_route": ROUTE, "orders_by_tier": PREMIUM, "open_sessions": SESSIONS},
    "two_updown": {"open_sessions": lifecycle_spec(("sessions.open", "sessions.open.again"), ("Business", "Enterprise"), limit=1)},
    "new_name": {"orders_by_tier": {"member": lambda r: r["schema"] == "OrderProcessing" and r["version"] in (1, 2) and r["fields"]["tier"] != "Enterprise",
                                    "dims": ("region", "tier"), "limit": 1000, "metrics": {"orders.nonenterprise": ("counter",)}}},
    "sessions_enterprise": {"open_sessions": lifecycle_spec(("sessions.open.enterprise",), ("Enterprise",))},
    "predicates": {
        "mid_amounts": {"member": lambda r: is_v2(r) and 500 <= int(r["fields"]["amount"]) <= 1199 and r["fields"]["provider"] != "Braintree"
                        and r.get("outcome") == "success", "dims": ("provider",), "limit": 1000, "metrics": {"mid.count": ("counter",)}},
        "either": {"member": lambda r: is_v2(r) and (r["fields"]["region"] == "DE" or r["fields"]["tier"] == "Enterprise"),
                   "dims": ("region",), "limit": 1000, "metrics": {"either.count": ("counter",)}},
        "transformed_present": {"member": lambda r: is_v2(r) and "note" in r["fields"] and "email" in r["fields"] and "tier" in r["fields"],
                                "dims": ("tier",), "limit": 1000, "metrics": {"present.count": ("counter",)}},
        "by_sku": {"member": lambda r: is_v2(r) and r["fields"]["sku"] in ("sku1", "sku3"), "dims": ("sku",), "limit": 1000, "metrics": {"sku.count": ("counter",)}},
        "discounts": {"member": is_v2, "dims": ("region",), "limit": 2, "metrics": {"discount.size": ("histogram", "discount", (100, 400))}},
        "by_discount": {"member": is_v2, "dims": ("discount",), "limit": 3, "metrics": {"discount.count": ("counter",)}},
    },
}


def first_arrivals(records):
    seen, out = set(), []
    for r in records:
        k = (r["id"]["producer"], r["id"]["seq"])
        if k not in seen:
            seen.add(k)
            out.append(r)
    return out


def oracle_projection(spec, records):
    """The expected summary of one projection over records in application order: every admitted record takes its
    tuple's series in that order, up to the limit, the rest the overflow series (design 3.6)."""
    if spec.get("lifecycle"):
        return oracle_lifecycle(spec, records)
    alloc, vals = [], {m: {} for m in spec["metrics"]}
    over = {m: None for m in spec["metrics"]}
    missing = {m: 0 for m, how in spec["metrics"].items() if how[0] == "histogram"}
    over_records = 0
    for r in records:
        if not spec["member"](r):
            continue
        key = json.dumps([r["fields"].get(d) for d in spec["dims"]])
        if key not in alloc and len(alloc) < spec["limit"]:
            alloc.append(key)
        slot = key if key in alloc else None
        over_records += slot is None
        for m, how in spec["metrics"].items():
            if how[0] == "counter":
                if slot:
                    vals[m][slot] = vals[m].get(slot, 0) + 1
                else:
                    over[m] = (over[m] or 0) + 1
                continue
            if how[1] not in r["fields"]:
                missing[m] += 1
                continue
            v = Decimal(r["fields"][how[1]])
            h = (vals[m].get(slot) if slot else over[m]) or {"count": 0, "sum": "0", "buckets": [0] * (len(how[2]) + 1)}
            h["count"] += 1
            h["sum"] = str(Decimal(h["sum"]) + v)
            h["buckets"][next((i for i, b in enumerate(how[2]) if v <= b), len(how[2]))] += 1
            if slot:
                vals[m][slot] = h
            else:
                over[m] = h
    return {"metrics": vals, "overflow": over, "allocated": sorted(alloc), "missing": missing, "overflow_records": over_records}


def oracle_lifecycle(spec, records):
    """Design 3.5's table, implemented here from the design, per metric, over records in application order. A member
    start that opens or closes a lifecycle takes its tuple's series, up to the projection's limit; past it, the
    overflow series (counted once per record however many metrics there are)."""
    limit = spec.get("limit", 1000)
    out = {"metrics": {}, "overflow": {}, "allocated": [], "anomalies": {}, "missing": {}, "overflow_records": 0}
    alloc, overflowed = [], set()
    for m in spec["metrics"]:
        table, bal, an = {}, {}, {}
        for r in records:
            if r["schema"] != "Session":
                continue
            start, end = r["fields"].get("phase") == "1", r["fields"].get("phase") == "2"
            if not (start or end):
                continue
            k = r["fields"].get("session")
            if k is None:
                an["missing_key"] = an.get("missing_key", 0) + 1
                continue
            lk = (r["id"]["producer"], k)
            st = table.get(lk)
            if start:
                if st is None or st[0] == "ended-first":
                    bound = None
                    if spec["start_member"](r):
                        key = json.dumps([r["fields"].get(d) for d in spec["dims"]])
                        if key not in alloc and len(alloc) < limit:
                            alloc.append(key)
                        bound = key if key in alloc else "__overflow__"
                        if bound == "__overflow__":
                            overflowed.add((r["id"]["producer"], r["id"]["seq"]))
                        bal.setdefault(bound, 0)
                        if st is None:
                            bal[bound] += 1
                    table[lk] = ("open" if st is None else "closed", bound)
                else:
                    c = "duplicate_start" if st[0] == "open" else "reuse"
                    an[c] = an.get(c, 0) + 1
            else:
                if st is None:
                    table[lk] = ("ended-first", None)
                elif st[0] == "open":
                    if st[1] is not None:
                        bal[st[1]] -= 1
                    table[lk] = ("closed", st[1])
                else:
                    an["duplicate_end"] = an.get("duplicate_end", 0) + 1
        out["overflow"][m] = bal.pop("__overflow__", None)
        out["metrics"][m], out["anomalies"][m] = bal, an
    out["allocated"] = sorted(alloc)
    out["overflow_records"] = len(overflowed)
    return out


def oracle(plan_key, records):
    recs = first_arrivals(records)
    return {name: oracle_projection(spec, recs) for name, spec in ORACLE[plan_key].items()}


def summarize(export, names):
    out = {}
    for name, p in export["projections"].items():
        if name not in names:
            continue
        out[name] = {"metrics": {m: {json.dumps(d): v for d, v in mv["series"]} for m, mv in p["metrics"].items()},
                     "overflow": {m: mv["overflow"] for m, mv in p["metrics"].items()},
                     "allocated": sorted(json.dumps(k) for k in p["allocated"]), "missing": p["missing"], "overflow_records": p["overflow_records"]}
        if p["anomalies"]:
            out[name]["anomalies"] = p["anomalies"]
    return out


def compare(name, export, plan_key, lines, formula, names=None):
    want = {k: v for k, v in oracle(plan_key, by_identity(lines, formula)).items() if not names or k in names}
    got = summarize(export, set(want))
    diff = {p: {"want": want[p], "got": got.get(p)} for p in want if got.get(p) != want[p]}
    return check(name, not diff, diff or None)


# ---------------------------------------------------------------- running things

class Runner:
    """Backend runs; every stream each run writes is kept under out/ for the egress scan."""

    def __init__(self, out):
        self.out, self.n = out, 0

    def __call__(self, backend, prog, telemetry=None, tag=""):
        env = {k: v for k, v in os.environ.items() if k != "FORTIS_TELEMETRY"}
        if telemetry:
            env["FORTIS_TELEMETRY"] = telemetry
        cmd = {"interp": [FORTIS, "run", prog], "vm": [FORTISC, "vm", prog], "native": [FORTISC, "native", prog]}[backend]
        r = subprocess.run(cmd, capture_output=True, cwd=ROOT, env=env, timeout=3600)
        self.n += 1
        base = os.path.join(self.out, "streams", f"{self.n:03d}-{backend}-{tag or 'run'}")
        os.makedirs(os.path.dirname(base), exist_ok=True)
        open(base + ".stdout", "wb").write(r.stdout)
        open(base + ".stderr", "wb").write(r.stderr)
        return r.returncode, r.stdout, r.stderr


def deliver(pipe_factory, batches, reorder_retry=False, kill=False):
    """At-least-once delivery: a batch is resent until acknowledged, and a crash restarts the pipeline (recovery).
    After a crash the batch is resent, when asked, reversed and split into single records. `kill` makes the crash a
    process kill (nothing on the disk is lost); otherwise it is a power loss. Returns the pipeline, the crashes, and
    how many batches were acknowledged (it stops at the first batch the pipeline does not acknowledge)."""
    pipe, i, crashes, retry = None, 0, 0, False
    while i < len(batches):
        try:
            if pipe is None:
                pipe = pipe_factory()
            if retry and reorder_retry:
                if not all([pipe.process([ln]) for ln in batches[i][::-1]]):
                    break
            elif not pipe.process(batches[i]):
                break
            i, retry = i + 1, False
        except Crash:
            pipe, retry = None, True
            crashes += 1
            if kill:
                pipe_factory.disk.crash_at = None
            else:
                pipe_factory.disk.crash()
    if pipe is None:
        pipe = pipe_factory()
    return pipe, crashes, i


RECORDING = [None]          # the real disk every pipeline in the run writes through, recording every byte


def P(state, registry, plan_text, disk=None, **kw):
    return Pipeline(state, registry, plan_text, disk=disk if disk is not None else RECORDING[0], **kw)


class Factory:
    def __init__(self, state, registry, plan_text, disk=None, **kw):
        self.state, self.registry, self.plan_text, self.disk, self.kw = state, registry, plan_text, disk, kw

    def __call__(self):
        return P(self.state, self.registry, self.plan_text, disk=self.disk, **self.kw)


def fresh_disk(crash_at=None):
    """A simulated disk on which the state directory and its parent already exist, unsynced, as a process killed
    before their parents' syncs would leave them (the pipeline must make every ancestor durable itself)."""
    disk = SimDisk()
    disk.makedirs("/sim")
    disk.makedirs("/sim/state")
    disk.ops, disk.crash_at = 0, crash_at
    return disk


def batches_of(lines, size):
    return [[ln.encode() if isinstance(ln, str) else ln for ln in lines[i:i + size]] for i in range(0, len(lines), size)]


def sim_files(disk):
    return {os.path.join(d, n): ino.data for d, dd in disk.dirs.items() for n, ino in dd["cur"].items()}


def archive_lines_real(d):
    out = []
    for n in sorted(os.listdir(d)) if os.path.isdir(d) else []:
        if n.startswith("seg-"):
            out += open(os.path.join(d, n), "rb").read().splitlines()
    return out


def archive_lines_sim(disk, d):
    return [ln for n in disk.listdir(d) if n.startswith("seg-") for ln in disk.read(os.path.join(d, n)).splitlines()]


def exactly_once(lines, expect_n):
    ids = [(json.loads(x)["id"]["producer"], json.loads(x)["id"]["seq"]) for x in lines]
    return len(ids) == len(set(ids)) == expect_n


def tree_bytes(d):
    out = {}
    for base, _, files in os.walk(d):
        for f in files:
            p = os.path.join(base, f)
            out[os.path.relpath(p, d)] = open(p, "rb").read()
    return out


def canary_patterns(core: str):
    """The raw core and its encodings: case, hex (both cases), base64 and URL-safe base64 at each alignment, percent
    encoding (both cases), JSON `\\u` escapes (both cases), reversal, and every 8-character piece of the core."""
    b = core.encode()
    pats = {core, core.upper(), core[::-1], b.hex(), b.hex().upper(),
            "".join(f"%{x:02x}" for x in b), "".join(f"%{x:02X}" for x in b),
            "".join(f"\\u{x:04x}" for x in b), "".join(f"\\u{x:04X}" for x in b)}
    for pad in range(3):
        for enc in (base64.b64encode, base64.urlsafe_b64encode):
            e = enc(b"x" * pad + b).decode()
            pats.add(e[(pad * 4 + 2) // 3 + 1:-4])
    pats |= {core[i:i + 8] for i in range(len(core) - 7)}
    return sorted(p for p in pats if len(p) >= 8)


def new_core(rnd, tag, n=24):
    return "v" + tag + "".join(rnd.choice(CANARY_ALPHABET) for _ in range(n))


def variant(path, cores, out):
    s = open(path, encoding="utf-8").read()
    for name, k in (("coreEmail", "email"), ("coreNote", "note"), ("coreCard", "card")):
        s, n = re.subn(rf'const {name}: String = "[^"]*"', f'const {name}: String = "{cores[k]}"', s)
        assert n == 1, name
    with open(out, "w", encoding="utf-8") as f:
        f.write(s)
    return out


# ---------------------------------------------------------------- the run

def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--json")
    ap.add_argument("--seed", type=int, default=9)
    ap.add_argument("--permutations", type=int, default=5)
    ap.add_argument("--crash-stride", type=int, default=1)
    ap.add_argument("--kills", type=int, default=4)
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args(argv)
    work = tempfile.mkdtemp(prefix="sp09-")
    inputs, out = os.path.join(work, "inputs"), os.path.join(work, "out")      # out/ is everything the egress scan reads
    os.makedirs(inputs)
    os.makedirs(out)
    sys.stdout = Tee(sys.__stdout__, os.path.join(out, "harness.log"))
    sys.stderr = Tee(sys.__stderr__, os.path.join(out, "harness.log"))
    started = time.time()
    run = Run(a, inputs, out)
    try:
        run.run()
    finally:
        summary = {"seed": a.seed, "permutations": a.permutations, "crash_stride": a.crash_stride, "kills": a.kills, "complete": run.complete,
                   "passed": sum(r["pass"] for r in RESULTS), "failed": sum(not r["pass"] for r in RESULTS),
                   "seconds": round(time.time() - started, 1), "results": RESULTS}
        if a.json:
            with open(a.json, "w", encoding="utf-8") as f:
                json.dump(summary, f, indent=1, sort_keys=True, default=str)
        print(f"{summary['passed']} passed, {summary['failed']} failed ({summary['seconds']} s)")
        sys.stdout, sys.stderr = sys.__stdout__, sys.__stderr__
        if a.keep:
            print(f"kept {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)
    return 0 if run.complete and RESULTS and not any(not r["pass"] for r in RESULTS) else 1


class Run:
    SECTIONS = (("B1", "backends"), ("C1", "clean"), ("P3", "keyed_hash"), ("P2", "paired"), ("D1", "identity"), ("D2", "deliveries"),
                ("D3", "crashes"), ("D3", "kills"), ("D4", "replay"), ("U1", "lifecycles"), ("E1", "epochs"), ("V1", "versions"),
                ("A1", "archive"), ("P4", "adversarial"), ("L1", "validation"), ("P1", "egress"))

    # every check each section must produce; a section that ends without an exception and without one of these is
    # incomplete evidence (`<FAMILY>.incomplete`), which the preregistration treats apart from a failure
    CHECKS = {
        "backends": [f"B1.{b}.runs" for b in BACKENDS] + [f"P1.{b}.diagnostic-plane-withholds" for b in BACKENDS] + ["B1.channels-identical", "B1.channel-equals-oracle", "S1.written"],
        "clean": ["B1.pipeline-outputs-identical", "C1.clean-pass", "C1.limit", "A1.archive-holds-every-valid-record", "A1.archive-holds-records-every-projection-rejects", "S1.equations", "U1.final-balances-zero"],
        "keyed_hash": ["P3.known-answers", "P3.joins-and-keys"],
        "paired": ["P2.dropped-and-redacted-values-invisible", "P2.diagnostic-streams-identical", "P2.hashed-values-every-backend-and-stream", "P2.hashed-values-differ-only-in-hashes",
                   "P2.cut-of-a-transformed-field-untold"],
        "identity": ["D1.identical-payloads-distinct-producers", "D1.both-archived", "D1.conflict-keeps-first", "D1.restarts-new-incarnation"],
        "deliveries": ["D2.twice", "D2.reversed", "D2.reversed-archive", "C1.permutations-change-allocation", "D2.gaps-filled-late", "D2.gaps-archive", "{permutations}"],
        "crashes": ["D3.simulated-clean-equals-real", "D3.every-crash-point", "D3.every-crash-point-reordered-redelivery", "D3.every-crash-point-archive-off",
                    "D3.every-crash-point-kill-then-power-loss"],
        "kills": ["D3.sigkill"],
        "replay": ["D4.replay-live-twice", "D4.replay-isolated-equals-live", "D4.replay-isolated-equals-oracle", "D4.replay-coverage", "D4.corrupt-segment-refused"],
        "lifecycles": ["U1.balances-and-anomalies-every-prefix", "U1.every-anomaly-code-exercised", "U1.membership-moves-both-ways", "U1.hashed-key-channel", "U1.hashed-key-lifecycles", "U1.two-updown-metrics-independent"],
        "epochs": ["E1.compatible-reload-keeps-epoch", "E1.reload_same_name-rejected", "E1.reload_privacy-rejected", "E1.rejected-reloads-change-nothing", "E1.new-epoch-caught-up",
                   "E1.new-epoch-coverage-complete", "E1.rollback-catches-up", "E1.new-version-may-change-privacy", "E1.lifecycle-open-across-reload", "E1.lifecycle-spanning-reload",
                   "E1.lifecycle-rollback", "E1.no-archive-first-activation-complete", "E1.no-archive-counts-since-activation", "E1.no-archive-coverage-stated", "E1.coverage-after-drops-partial"],
        "versions": ["V1.v1-channel-equals-oracle", "V1.mixed-producers", "V1.versions-kept", "V1.unpinned-admits-compatible-versions", "V1.replay-after-registry-upgrade"],
        "archive": ["A1.archive-off-same-metrics", "A1.block-then-room", "A1.drop-keeps-metrics", "A1.drop-is-final", "A1.replay-under-other-privacy-refused"],
        "adversarial": ["P4.quarantine-codes", "S1.equations-with-quarantine"],
        "validation": ["L1.codes-at-lines", "L1.yaml-quoted-scalars", "L1.predicates-select-by-value"],
        "egress": ["P1.no-canary-in-any-output"],
    }

    def __init__(self, a, inputs, out):
        self.a, self.inputs, self.out = a, inputs, out
        self.rnd = random.Random(a.seed)
        self.backend = Runner(out)
        self.complete = False
        RECORDING[0] = RealDisk(record=os.path.join(out, "writes.log"))

    def expected(self, name):
        out = []
        for c in self.CHECKS[name]:
            out += [x for j in range(self.a.permutations) for x in (f"D2.permutation-{j}", f"D2.permutation-{j}-archive")] if c == "{permutations}" else [c]
        return out

    def run(self):
        try:
            self.setup()
        except Exception as e:                              # noqa: BLE001
            check("SETUP.exception", False, {"error": f"{type(e).__name__}: {e}"})
            return
        incomplete = False
        for family, name in self.SECTIONS:
            first = len(RESULTS)
            try:
                getattr(self, name)()
            except Exception as e:                          # noqa: BLE001  (an exception is a result of its family)
                check(f"{family}.exception", False, {"section": name, "error": f"{type(e).__name__}: {e}", "where": traceback.format_exc().splitlines()[-3:]})
                continue
            missing = sorted(set(self.expected(name)) - {r["check"] for r in RESULTS[first:]})
            if missing:
                incomplete = True
                check(f"{family}.incomplete", False, {"section": name, "missing": missing})
        self.complete = not incomplete

    def setup(self):
        a, rnd = self.a, self.rnd
        self.key1 = hashlib.sha256(f"sp09-key1-{a.seed}".encode()).hexdigest()
        self.key2 = hashlib.sha256(f"sp09-key2-{a.seed}".encode()).hexdigest()
        self.P = hashlib.sha256(f"sp09-producer-p-{a.seed}".encode()).hexdigest()[:32]
        self.Q = hashlib.sha256(f"sp09-producer-q-{a.seed}".encode()).hexdigest()[:32]
        self.cores = {"a": {k: new_core(rnd, k[0]) for k in ("email", "note", "card")}}
        ca = self.cores["a"]
        # P2: B's dropped and redacted values differ and are all longer than their bound; C's hashed values differ
        # with the same equalities; D's hashed values are all longer than their bound
        self.cores["b"] = dict(ca, note=new_core(rnd, "n", 70), card=new_core(rnd, "d", 70))
        self.cores["c"] = dict(ca, email=new_core(rnd, "e"))
        self.cores["d"] = dict(ca, email=new_core(rnd, "e", 70))
        self.probe = new_core(rnd, "p")                                                 # P4's crafted canary
        self.all_cores = sorted({c for v in self.cores.values() for c in v.values()} | {self.probe})
        self.progs = {t: variant(WORKLOAD, c, os.path.join(self.inputs, f"orders_{t}.ft")) for t, c in self.cores.items()}
        self.prog_v1 = variant(WORKLOAD_V1, ca, os.path.join(self.inputs, "orders_v1_a.ft"))
        self.registry = Registry()
        self.registry.add_program([self.prog_v1])
        self.registry.add_program([self.progs["a"]])
        self.reg_path = os.path.join(self.inputs, "registry.json")
        self.registry.save(self.reg_path)
        self.plan_text = {f[:-5]: open(os.path.join(PLANS, f), encoding="utf-8").read() for f in os.listdir(PLANS) if f.endswith(".yaml")}
        self.main_plan = planmod.load(self.plan_text["main"], self.registry)
        self.F = {"P2": formula_map(expected_channel(2, ca, self.key1, "k1", self.P)), "Q2": formula_map(expected_channel(2, ca, self.key1, "k1", self.Q)),
                  "Q1": formula_map(expected_channel(1, ca, self.key1, "k1", self.Q))}
        self.expect_a = expected_channel(2, ca, self.key1, "k1", self.P)
        v3 = open(WORKLOAD, encoding="utf-8").read().replace("  version: 2\n", "  version: 3\n").replace(
            "  discount: Int                      required: none\n", "  discount: Int                      required: none\n  channel: Int                       required: none\n")
        p3 = os.path.join(self.inputs, "orders_v3.ft")
        open(p3, "w").write(v3)
        self.reg3 = Registry.from_json(self.registry.to_json())
        self.reg3.add_program([p3])

    def policy(self, name, channel, producer=None, key=None, key_id="k1", plan=None):
        path = os.path.join(self.inputs, name)
        with open(path, "w") as f:
            f.write(planmod.compile_policy(plan or self.main_plan, self.registry, key_id, key or self.key1, channel, producer))
        return path

    def channel_of(self, backend, prog, tag, producer=None, **kw):
        ch = os.path.join(self.out, f"channel-{tag}.jsonl")
        code, so, se = self.backend(backend, prog, self.policy(f"policy-{tag}", ch, producer, **kw), tag)
        self.last = (so, se)
        return code, so, (open(ch, "rb").read() if os.path.exists(ch) else b"")

    def sim_run(self, lines, size=10, plan_key="main", reg=None):
        disk = SimDisk()
        pipe, _, _ = deliver(Factory("/sim", reg or self.registry, self.plan_text[plan_key], disk=disk), batches_of(lines, size))
        return pipe, disk

    # -- B1 and the adapter: each backend under the policy (incarnation pinned), and with no policy at all
    def backends(self):
        self.channels, self.streams_a = {}, {}
        for b in BACKENDS:
            code, so, ch = self.channel_of(b, self.progs["a"], f"{b}-a", self.P)
            self.channels[b] = ch
            self.streams_a[(b, "policy")] = self.last
            check(f"B1.{b}.runs", code == 0 and b"240 orders, 63 sessions" in so, {"code": code})
            code, so, se = self.backend(b, self.progs["a"], None, f"{b}-default")
            self.streams_a[(b, "default")] = (so, se)
            diag = [ln for ln in (so + se).decode(errors="replace").splitlines() if "observation-" in ln]
            withheld = all(('"fields": {}' in ln) or ("fields=;" in ln) or ln.rstrip().endswith("fields=") for ln in diag)
            check(f"P1.{b}.diagnostic-plane-withholds", code == 0 and len(diag) == 393 and withheld, {"records": len(diag), "sample": diag[:1]})
        check("B1.channels-identical", self.channels["interp"] == self.channels["vm"] == self.channels["native"], {b: len(c) for b, c in self.channels.items()})
        got = self.channels["interp"].decode().splitlines()
        check("B1.channel-equals-oracle", got == self.expect_a, {"first-difference": next((i for i, (x, y) in enumerate(zip(got, self.expect_a)) if x != y), None),
                                                                  "lines": [len(got), len(self.expect_a)]})
        self.lines = got
        check("S1.written", len(got) == 393, len(got))

    # -- the clean pass (real disk), and every backend's channel through the pipeline
    def clean(self):
        states = {}
        for b in BACKENDS:
            st = os.path.join(self.out, f"state-{b}")
            pipe, _, _ = deliver(Factory(st, self.registry, self.plan_text["main"]), batches_of(self.channels[b].decode().splitlines(), 10))
            states[b] = (pipe.export(), tree_bytes(st))
        self.states = states
        self.e0 = states["interp"][0]
        e0, lines = self.e0, self.lines
        check("B1.pipeline-outputs-identical", states["interp"] == states["vm"] == states["native"])
        compare("C1.clean-pass", e0, "main", lines, self.F["P2"])
        route_e = e0["projections"]["orders_by_route"]
        check("C1.limit", len(route_e["allocated"]) == 20 and all(len(m["series"]) <= 20 for m in route_e["metrics"].values()) and route_e["overflow_records"] > 0,
              {"allocated": len(route_e["allocated"]), "overflow_records": route_e["overflow_records"]})
        arch = [json.loads(x) for x in archive_lines_real(os.path.join(self.out, "state-interp", "archive"))]
        by_schema = {}
        for r in arch:
            by_schema[r["schema"]] = by_schema.get(r["schema"], 0) + 1
        check("A1.archive-holds-every-valid-record", len(arch) == len({(r["id"]["producer"], r["id"]["seq"]) for r in arch}) == 393 and by_schema.get("Audit") == 24, by_schema)
        formula = list(self.F["P2"].values())
        rejected = sum(1 for r in formula if r["schema"] == "OrderProcessing" and not route(r) and not premium(r))
        check("A1.archive-holds-records-every-projection-rejects",
              rejected > 0 and sum(1 for r in arch if r["schema"] == "OrderProcessing" and not route(r) and not premium(r)) == rejected, rejected)
        led = e0["ledger"]
        cnt, amt = route_e["metrics"]["orders.count"], route_e["metrics"]["orders.amount"]
        check("S1.equations", led["quarantine"] == {} and led["malformed_deliveries"] == 0 and led["archive_dropped"] == 0 and led["accepted"] == 393
              and e0["archive"]["records"] == 393
              and route_e["admitted"] == sum(v for _, v in cnt["series"]) + (cnt["overflow"] or 0)
              and route_e["admitted"] == sum(v["count"] for _, v in amt["series"]) + (amt["overflow"] or {"count": 0})["count"] + route_e["missing"]["orders.amount"],
              {"ledger": led, "archive": e0["archive"]})
        check("U1.final-balances-zero", all(v == 0 for _, v in e0["projections"]["open_sessions"]["metrics"]["sessions.open"]["series"]))

    # -- P3: the keyed hash
    def keyed_hash(self):
        py_rfc = hmac.new(b"Jefe", b"what do ya want for nothing?", hashlib.sha256).hexdigest() == "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        cargo = subprocess.run(["cargo", "test", "-q", "--offline", "-p", "fortis-runtime", "telemetry"], cwd=os.path.join(ROOT, "compiler"), capture_output=True, text=True, timeout=3600)
        check("P3.known-answers", py_rfc and cargo.returncode == 0 and "4 passed" in cargo.stdout, cargo.stdout[-300:])
        recs = [json.loads(x) for x in self.lines]
        emails = [r["fields"]["email"] for r in recs if r["schema"] == "OrderProcessing"]
        pattern_ok = all((emails[i] == emails[j]) == (i % 40 == j % 40) for i in range(0, 240, 7) for j in range(0, 240, 5))
        _, _, ch2 = self.channel_of("interp", self.progs["a"], "key2", self.P, key=self.key2, key_id="k2")
        k2 = [json.loads(x) for x in ch2.decode().splitlines()]
        disjoint = {r["fields"]["email"].split(":")[2] for r in k2 if r["schema"] == "OrderProcessing"}.isdisjoint({e.split(":")[2] for e in emails})
        check("P3.joins-and-keys", pattern_ok and disjoint and len(k2) == 393 and all(r["fields"]["email"].startswith("h1:k2:") for r in k2 if r["schema"] == "OrderProcessing"))

    # -- P2: paired inputs, on every backend, across the bound
    def paired(self):
        base = (self.channels["interp"], self.states["interp"][1], self.e0)
        same, streams = {}, {}

        def norm(pair):
            return tuple(x.replace(b"orders_b.ft", b"orders_a.ft") for x in pair)

        for b in BACKENDS:
            _, _, chb = self.channel_of(b, self.progs["b"], f"{b}-b", self.P)
            same[b] = chb == self.channels[b]
            streams[(b, "policy")] = norm(self.last) == self.streams_a[(b, "policy")]
            _, so, se = self.backend(b, self.progs["b"], None, f"{b}-b-default")
            streams[(b, "default")] = norm((so, se)) == self.streams_a[(b, "default")]
        check("P2.diagnostic-streams-identical", all(streams.values()), {f"{k[0]}-{k[1]}": v for k, v in streams.items()})
        st = os.path.join(self.out, "state-b")
        pipe, _, _ = deliver(Factory(st, self.registry, self.plan_text["main"]), batches_of(self.channels["interp"].decode().splitlines(), 10))
        check("P2.dropped-and-redacted-values-invisible", all(same.values()) and (self.channels["interp"], tree_bytes(st), pipe.export()) == base, same)

        def unhash(blob):
            # keyed hashes by order of first appearance; the manifest's segment digests digest segments holding them
            seen = {}
            blob = re.sub(rb"h1:k1:[0-9a-f]{32}", lambda m: b"h1:k1:#%d" % seen.setdefault(m.group(0), len(seen)), blob)
            return re.sub(rb'"sha256": "[0-9a-f]{64}"', b'"sha256": "#"', blob)

        cstreams = {}

        def normc(pair):
            return tuple(x.replace(b"orders_c.ft", b"orders_a.ft") for x in pair)

        for b in BACKENDS:
            _, _, chcb = self.channel_of(b, self.progs["c"], f"{b}-c-paired", self.P)
            cstreams[(b, "channel")] = unhash(chcb) == unhash(self.channels[b]) and chcb != self.channels[b]
            cstreams[(b, "policy")] = normc(self.last) == self.streams_a[(b, "policy")]
            _, so, se = self.backend(b, self.progs["c"], None, f"{b}-c-default")
            cstreams[(b, "default")] = normc((so, se)) == self.streams_a[(b, "default")]
        check("P2.hashed-values-every-backend-and-stream", all(cstreams.values()), {f"{k[0]}-{k[1]}": v for k, v in cstreams.items()})
        _, _, chc = self.channel_of("interp", self.progs["c"], "interp-c", self.P)
        st = os.path.join(self.out, "state-c")
        pipe, _, _ = deliver(Factory(st, self.registry, self.plan_text["main"]), batches_of(chc.decode().splitlines(), 10))
        tc = tree_bytes(st)
        check("P2.hashed-values-differ-only-in-hashes", unhash(chc) == unhash(base[0]) and chc != base[0]
              and {k: unhash(v) for k, v in tc.items()} == {k: unhash(v) for k, v in base[1].items()} and pipe.export() == self.e0)
        cuts = {}
        for b in BACKENDS:
            _, _, chd = self.channel_of(b, self.progs["d"], f"{b}-d", self.P)
            recs = [json.loads(x) for x in chd.decode().splitlines()]
            emails = {r["fields"]["email"] for r in recs if r["schema"] == "OrderProcessing"}
            cuts[b] = len(recs) == 393 and len(emails) == 1 and all(r["truncated"] == [] for r in recs)
            code, so, se = self.backend(b, self.progs["d"], None, f"{b}-d-default")
            cuts[b] = cuts[b] and "truncated\": [\"" not in (so + se).decode(errors="replace")
        check("P2.cut-of-a-transformed-field-untold", all(cuts.values()), cuts)

    # -- D1: identity
    def identity(self):
        lines = self.lines
        _, _, chq = self.channel_of("interp", self.progs["a"], "q", self.Q)
        q_lines = chq.decode().splitlines()
        pipe, _, _ = deliver(Factory(os.path.join(self.out, "state-pq"), self.registry, self.plan_text["main"]), batches_of(lines + q_lines, 10))
        compare("D1.identical-payloads-distinct-producers", pipe.export(), "main", lines + q_lines, {**self.F["P2"], **self.F["Q2"]})
        check("D1.both-archived", exactly_once(archive_lines_real(os.path.join(self.out, "state-pq", "archive")), 786))
        conflict = json.loads(lines[4])
        conflict["fields"]["amount"] = "123456"
        pipe, _, _ = deliver(Factory(os.path.join(self.out, "state-conflict"), self.registry, self.plan_text["main"]), batches_of(lines + [json.dumps(conflict, sort_keys=True)], 10))
        ex = pipe.export()
        arch = archive_lines_real(os.path.join(self.out, "state-conflict", "archive"))
        check("D1.conflict-keeps-first", ex["ledger"]["identity_conflicts"] == 1 and ex["projections"] == self.e0["projections"] and lines[4].encode() in arch and exactly_once(arch, 393))
        incarnations = set()
        for b in BACKENDS:
            for i in range(2 if b != "native" else 1):
                _, _, ch = self.channel_of(b, self.progs["a"], f"default-{b}-{i}")
                incarnations.add(json.loads(ch.decode().splitlines()[0])["id"]["producer"])
        check("D1.restarts-new-incarnation", len(incarnations) == 5 and all(re.fullmatch(r"[0-9a-f]{32}", x) for x in incarnations), sorted(incarnations))

    # -- D2: deliveries (simulated disk, no crash)
    def deliveries(self):
        lines, F, rnd = self.lines, self.F["P2"], self.rnd
        pipe, disk = self.sim_run(lines + lines)
        check("D2.twice", pipe.export() == self.e0 and exactly_once(archive_lines_sim(disk, "/sim/archive"), 393))
        rev = lines[::-1]
        pipe, disk = self.sim_run(rev)
        compare("D2.reversed", pipe.export(), "main", rev, F)
        check("D2.reversed-archive", exactly_once(archive_lines_sim(disk, "/sim/archive"), 393))
        crossing = {tuple(summarize(self.e0, {"orders_by_route"})["orders_by_route"]["allocated"]),
                    tuple(summarize(pipe.export(), {"orders_by_route"})["orders_by_route"]["allocated"])}
        for j in range(self.a.permutations):
            order = lines[:]
            rnd.shuffle(order)
            for _ in range(len(order) // 5):
                order.insert(rnd.randrange(len(order) + 1), rnd.choice(lines))
            pipe, disk = self.sim_run(order, size=rnd.choice((1, 7, 25)))
            ex = pipe.export()
            compare(f"D2.permutation-{j}", ex, "main", order, F)
            check(f"D2.permutation-{j}-archive", exactly_once(archive_lines_sim(disk, "/sim/archive"), 393))
            crossing.add(tuple(summarize(ex, {"orders_by_route"})["orders_by_route"]["allocated"]))
        check("C1.permutations-change-allocation", len(crossing) > 1, len(crossing))
        gap1 = [ln for i, ln in enumerate(lines) if (i + 1) % 7 != 0]
        gap2 = [ln for i, ln in enumerate(lines) if (i + 1) % 7 == 0]
        disk = SimDisk()
        deliver(Factory("/sim", self.registry, self.plan_text["main"], disk=disk), batches_of(gap1, 10))
        pipe, _, _ = deliver(Factory("/sim", self.registry, self.plan_text["main"], disk=disk), batches_of(gap2 + gap1[:30], 10))   # a restart, then late records and repeats
        compare("D2.gaps-filled-late", pipe.export(), "main", gap1 + gap2, F)
        check("D2.gaps-archive", exactly_once(archive_lines_sim(disk, "/sim/archive"), 393))

    # -- D3: a crash after every disk operation (torn final writes, lost directory entries), then recovery and redelivery
    def crashes(self):
        lines, F = self.lines, self.F["P2"]
        batches = batches_of(lines, 25)
        dry = fresh_disk()
        pipe, _, _ = deliver(Factory("/sim/state", self.registry, self.plan_text["main"], disk=dry), batches)
        self.total_ops = dry.ops
        check("D3.simulated-clean-equals-real", pipe.export() == self.e0, self.total_ops)
        for plan_key, tag, reorder, kill in (("main", "", False, False), ("main", "-reordered-redelivery", True, False), ("archive_off", "-archive-off", False, False),
                                             ("main", "-kill-then-power-loss", False, True)):
            dry = fresh_disk()
            pipe, _, _ = deliver(Factory("/sim/state", self.registry, self.plan_text[plan_key], disk=dry), batches)
            clean, ops = pipe.export(), dry.ops
            failures, cut = [], 0
            for k in range(1, ops + 1, self.a.crash_stride):
                disk = fresh_disk(k)
                try:
                    pipe, crashes, _ = deliver(Factory("/sim/state", self.registry, self.plan_text[plan_key], disk=disk), batches, reorder_retry=reorder, kill=kill)
                    if kill:
                        # everything was acknowledged after a process kill and a recovery: a power loss now loses nothing
                        disk.crash()
                        pipe = P("/sim/state", self.registry, self.plan_text[plan_key], disk=disk)
                    ex = pipe.export()
                    if plan_key == "archive_off":
                        ok = ex == clean
                    else:
                        arch = archive_lines_sim(disk, "/sim/state/archive")
                        iso = P("/iso", self.registry, self.plan_text[plan_key], disk=disk, archive_override=False)
                        iso.replay("/sim/state/archive")
                        # the oracle over the archive's own order (application order is archive order, design 3.6)
                        want = oracle("main", by_identity([x.decode() for x in arch], F))
                        ok = exactly_once(arch, 393) and summarize(ex, set(want)) == want and iso.export()["projections"] == ex["projections"] \
                            and (reorder or ex == self.e0)
                except Exception as e:                      # noqa: BLE001  (a recovery failure is a result)
                    ok, crashes = False, f"{type(e).__name__}: {e}"
                cut += 1
                if not ok:
                    failures.append({"k": k, "crashes": crashes})
            check(f"D3.every-crash-point{tag}", not failures and cut > 0, {"points": cut, "total_ops": ops, "failures": failures[:10]})

    def kills(self):
        batches = batches_of(self.lines, 25)
        kills = sorted(self.rnd.sample(range(1, len(batches) - 1), min(self.a.kills, len(batches) - 2)))
        st = os.path.join(self.out, "state-kill")
        i, killed, stalled = 0, 0, []

        def send(p, data):
            """Write all of `data` before the deadline, or report the stall."""
            fd, view, deadline = p.stdin.fileno(), memoryview(data), time.time() + ACK_DEADLINE
            while view:
                _, w, _ = select.select([], [fd], [], max(0.0, deadline - time.time()))
                if not w:
                    return False
                view = view[os.write(fd, view):]
            return True

        def line(p, buf):
            """One line from the child before the deadline, or None."""
            fd, deadline = p.stdout.fileno(), time.time() + ACK_DEADLINE
            while b"\n" not in buf[0]:
                r, _, _ = select.select([fd], [], [], max(0.0, deadline - time.time()))
                if not r:
                    return None
                chunk = os.read(fd, 4096)
                if not chunk:
                    return None
                buf[0] += chunk
            ln, buf[0] = buf[0].split(b"\n", 1)
            return ln.decode(errors="replace").strip()

        def reap(p):
            p.kill()
            try:
                p.wait(timeout=ACK_DEADLINE)
            except subprocess.TimeoutExpired:
                stalled.append({"reap": "no exit after SIGKILL"})

        for k in kills + [None]:
            p = subprocess.Popen([sys.executable, "-m", "telemetry", "serve", st, self.reg_path, os.path.join(PLANS, "main.yaml")], stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=open(os.path.join(self.out, "serve.stderr"), "ab"), cwd=ROOT)
            os.set_blocking(p.stdin.fileno(), False)
            os.set_blocking(p.stdout.fileno(), False)
            buf = [b""]
            while i < len(batches):
                if not send(p, (json.dumps({"batch": i, "records": [x.decode() for x in batches[i]]}) + "\n").encode()):
                    stalled.append({"batch": i, "write": "stalled"})
                    break
                if k is not None and i == k:
                    time.sleep(self.rnd.random() * 0.02)
                    reap(p)
                    killed += 1
                    break
                ack = line(p, buf)
                if ack != f"ack {i}":
                    stalled.append({"batch": i, "ack": ack})
                    break
                i += 1
            if k is None or stalled:
                p.stdin.close()
                try:
                    p.wait(timeout=ACK_DEADLINE)
                except subprocess.TimeoutExpired:
                    stalled.append({"exit": "no exit"})
                    reap(p)
            if stalled:
                break
        pipe = P(st, self.registry, self.plan_text["main"])
        check("D3.sigkill", not stalled and killed == len(kills) and i == len(batches) and pipe.export() == self.e0
              and exactly_once(archive_lines_real(os.path.join(st, "archive")), 393), {"kills": kills, "delivered": i, "stalled": stalled})

    # -- D4: replay
    def replay(self):
        live = os.path.join(self.out, "state-interp")
        pipe = P(live, self.registry, self.plan_text["main"])
        pipe.replay(os.path.join(live, "archive"))
        pipe.replay(os.path.join(live, "archive"))
        check("D4.replay-live-twice", pipe.export() == self.e0)
        iso = P(os.path.join(self.out, "state-isolated"), self.registry, self.plan_text["main"], archive_override=False)
        iso.replay(os.path.join(live, "archive"))
        check("D4.replay-isolated-equals-live", iso.export()["projections"] == self.e0["projections"])
        compare("D4.replay-isolated-equals-oracle", iso.export(), "main", self.lines, self.F["P2"])
        # what a replay brought counts as accepted: a later epoch with no archive to catch up from says so
        later = P(os.path.join(self.out, "state-isolated"), self.registry, self.plan_text["reload_new_name"], archive_override=False)
        check("D4.replay-coverage", later.export()["projections"]["orders_by_tier"]["coverage"] == "since_activation")
        bad = os.path.join(self.out, "archive-corrupt")
        shutil.copytree(os.path.join(live, "archive"), bad)
        seg = os.path.join(bad, "seg-000003.jsonl")
        data = bytearray(open(seg, "rb").read())
        data[100] ^= 0x01
        open(seg, "wb").write(bytes(data))
        iso2 = P(os.path.join(self.out, "state-isolated-2"), self.registry, self.plan_text["main"], archive_override=False)
        try:
            iso2.replay(bad)
            refused = None
        except ArchiveRefused as e:
            refused = e.what
        check("D4.corrupt-segment-refused", refused == "seg-000003.jsonl" and all(p["admitted"] == 0 for p in iso2.export()["projections"].values()), refused)

    # -- U1: lifecycle balances and anomalies after every prefix of the delivery
    def lifecycles(self):
        lines, F, rnd = self.lines, self.F["P2"], self.rnd
        sess = [ln for ln in lines if '"schema": "Session"' in ln]
        orders = [("clean", sess), ("reversed", sess[::-1])] + [(f"perm-{j}", rnd.sample(sess, len(sess))) for j in range(self.a.permutations)]
        bad = []
        for tag, order in orders:
            pipe = P("/sim", self.registry, self.plan_text["main"], disk=SimDisk())
            for n in range(len(order)):
                pipe.process([order[n].encode()])
                e = pipe.epochs["open_sessions"].export()
                got = ({json.dumps(d): v for d, v in e["metrics"]["sessions.open"]["series"]}, e["anomalies"]["sessions.open"])
                w = oracle_lifecycle(SESSIONS, by_identity(order[:n + 1], F))
                want = (w["metrics"]["sessions.open"], w["anomalies"]["sessions.open"])
                if got != want or any(v < 0 for v in got[0].values()):
                    bad.append({"order": tag, "prefix": n + 1, "want": want, "got": got})
                    break
        check("U1.balances-and-anomalies-every-prefix", not bad, bad[:5])
        want = oracle_lifecycle(SESSIONS, by_identity(sess, F))["anomalies"]["sessions.open"]
        check("U1.every-anomaly-code-exercised", set(want) == {"duplicate_start", "duplicate_end", "reuse", "missing_key"}, want)
        moved_in = sum(1 for s in range(60) if tier(s) == "Basic" and end_tier(s) != "Basic")
        moved_out = sum(1 for s in range(60) if tier(s) != "Basic" and end_tier(s) == "Basic")
        check("U1.membership-moves-both-ways", moved_in > 0 and moved_out > 0, {"into": moved_in, "out of": moved_out})
        kh = planmod.load(self.plan_text["key_hashed"], self.registry)
        _, _, chk = self.channel_of("interp", self.progs["a"], "keyhashed", self.P, plan=kh)
        kl = chk.decode().splitlines()
        expect = expected_channel(2, self.cores["a"], self.key1, "k1", self.P, hash_session=True)
        check("U1.hashed-key-channel", kl == expect)
        pipe, _, _ = deliver(Factory(os.path.join(self.out, "state-keyhashed"), self.registry, self.plan_text["key_hashed"]), batches_of(kl, 10))
        compare("U1.hashed-key-lifecycles", pipe.export(), "main", kl, formula_map(expect), names={"open_sessions"})
        pipe, _ = self.sim_run(lines, plan_key="two_updown")
        compare("U1.two-updown-metrics-independent", pipe.export(), "two_updown", lines, F)

    # -- E1: epochs
    def epochs(self):
        lines, F, out = self.lines, self.F["P2"], self.out
        st = os.path.join(out, "state-epochs")
        half = len(lines) // 2
        deliver(Factory(st, self.registry, self.plan_text["main"]), batches_of(lines[:half], 10))
        hist0 = json.load(open(os.path.join(st, "plans.json")))
        P(st, self.registry, self.plan_text["reload_export"])
        check("E1.compatible-reload-keeps-epoch", hist0 == json.load(open(os.path.join(st, "plans.json"))))
        before = tree_bytes(st)
        for key, code in (("reload_same_name", "E-TEL-006"), ("reload_privacy", "E-TEL-007")):
            try:
                P(st, self.registry, self.plan_text[key])
                diag = None
            except planmod.PlanInvalid as e:
                diag = [(d.code, d.line) for d in e.diags]
            mark = next(i + 1 for i, ln in enumerate(self.plan_text[key].splitlines()) if f"expect after main.yaml: {code}" in ln)
            check(f"E1.{key}-rejected", diag == [(code, mark)], diag)
        check("E1.rejected-reloads-change-nothing", tree_bytes(st) == before)
        pipe, _, _ = deliver(Factory(st, self.registry, self.plan_text["reload_new_name"]), batches_of(lines[half:], 10))
        compare("E1.new-epoch-caught-up", pipe.export(), "new_name", lines, F)
        check("E1.new-epoch-coverage-complete", pipe.export()["projections"]["orders_by_tier"]["coverage"] == "complete")
        pipe = P(st, self.registry, self.plan_text["main"])
        compare("E1.rollback-catches-up", pipe.export(), "main", lines, F, names={"orders_by_tier"})
        # a new schema version may take new privacy actions, without touching the versions already taken
        P(st, self.reg3, self.plan_text["reload_privacy_v3"])
        hist = json.load(open(os.path.join(st, "privacy.json")))
        pol = planmod.compile_policy(planmod.load(self.plan_text["reload_privacy_v3"], self.reg3), self.reg3, "k1", self.key1, "/dev/null")
        check("E1.new-version-may-change-privacy", hist.get("OrderProcessing|3|sku") == "hash" and hist.get("OrderProcessing|2|sku") == "permit"
              and "field OrderProcessing 3 sku hash " in pol and "field OrderProcessing 2 sku permit " in pol, {k: v for k, v in hist.items() if k.endswith("|sku")})
        # a lifecycle spanning the reload: cut just after the start of an Enterprise session whose end comes later
        st = os.path.join(out, "state-epochs-sessions")
        cut = next(i + 1 for i, ln in enumerate(lines) if i > 300 and '"phase": "1"' in ln and '"tier": "Enterprise"' in ln and '"session"' in ln)
        deliver(Factory(st, self.registry, self.plan_text["main"]), batches_of(lines[:cut], 10))
        pipe = P(st, self.registry, self.plan_text["reload_sessions"])
        e = pipe.export()["projections"]["open_sessions"]["metrics"]["sessions.open.enterprise"]["series"]
        check("E1.lifecycle-open-across-reload", dict((json.dumps(d), v) for d, v in e) == {json.dumps(["Enterprise"]): 1}, e)
        pipe, _, _ = deliver(lambda: pipe, batches_of(lines[cut:], 10))
        compare("E1.lifecycle-spanning-reload", pipe.export(), "sessions_enterprise", lines, F)
        pipe = P(st, self.registry, self.plan_text["main"])
        compare("E1.lifecycle-rollback", pipe.export(), "main", lines, F, names={"open_sessions"})
        st = os.path.join(out, "state-epochs-noarchive")
        p0 = P(st, self.registry, self.plan_text["archive_off"])
        check("E1.no-archive-first-activation-complete", all(p["coverage"] == "complete" for p in p0.export()["projections"].values()))
        deliver(lambda: p0, batches_of(lines[:half], 10))
        noarch = self.plan_text["reload_new_name"].replace("    enabled: true\n", "    enabled: false\n")
        pipe, _, _ = deliver(Factory(st, self.registry, noarch), batches_of(lines[half:], 10))
        compare("E1.no-archive-counts-since-activation", pipe.export(), "new_name", lines[half:], F)
        check("E1.no-archive-coverage-stated", pipe.export()["projections"]["orders_by_tier"]["coverage"] == "since_activation")
        disk = SimDisk()
        pipe = P("/sim", self.registry, self.plan_text["archive_drop"], disk=disk)
        for b in batches_of(lines, 10):
            pipe.process(b)
        pipe = P("/sim", self.registry, self.plan_text["reload_new_name"].replace("    on_full: block", "    capacity_records: 200\n    on_full: drop"), disk=disk)
        check("E1.coverage-after-drops-partial", pipe.export()["projections"]["orders_by_tier"]["coverage"] == "archive_partial")

    # -- V1: versions
    def versions(self):
        lines, out = self.lines, self.out
        code, _, chv1 = self.channel_of("interp", self.prog_v1, "v1", self.Q)
        v1_lines = chv1.decode().splitlines()
        check("V1.v1-channel-equals-oracle", v1_lines == expected_channel(1, self.cores["a"], self.key1, "k1", self.Q))
        mixed = [x for pair in zip(lines, v1_lines[50:] + [None] * 50) for x in pair if x is not None] + v1_lines[:50]
        F = {**self.F["P2"], **self.F["Q1"]}
        pipe, disk = self.sim_run(mixed)
        ex = pipe.export()
        compare("V1.mixed-producers", ex, "main", mixed, F)
        arch = [json.loads(x) for x in archive_lines_sim(disk, "/sim/archive")]
        check("V1.versions-kept", sum(1 for r in arch if r["schema"] == "OrderProcessing" and r["version"] == 1) == 240
              and sum(1 for r in arch if r["schema"] == "OrderProcessing" and r["version"] == 2) == 240)
        pipe2, _ = self.sim_run(mixed, plan_key="unpinned_provider")
        check("V1.unpinned-admits-compatible-versions", summarize(pipe2.export(), {"orders_by_route"}) == summarize(ex, {"orders_by_route"}))
        reg3 = self.reg3
        copy = os.path.join(out, "archive-v1v2")
        os.makedirs(copy)
        for n, data in sim_files(disk).items():
            if n.startswith("/sim/archive/"):
                open(os.path.join(copy, os.path.basename(n)), "wb").write(data)
        up = P(os.path.join(out, "state-v3"), reg3, self.plan_text["main"], archive_override=False)
        up.replay(copy)
        check("V1.replay-after-registry-upgrade", up.export()["projections"] == ex["projections"])

    # -- A1: the archive disabled, full and blocking, full and dropping (a drop is final)
    def archive(self):
        lines = self.lines
        pipe, disk = self.sim_run(lines, plan_key="archive_off")
        check("A1.archive-off-same-metrics", pipe.export()["projections"] == self.e0["projections"] and not disk.is_dir("/sim/archive"))
        disk = SimDisk()
        blk = batches_of(lines, 10)
        pipe = P("/sim", self.registry, self.plan_text["archive_block"], disk=disk)
        acked = [pipe.process(b) for b in blk]
        stuck = [i for i, x in enumerate(acked) if not x]
        full_at = len(archive_lines_sim(disk, "/sim/archive"))
        pipe = P("/sim", self.registry, self.plan_text["main"], disk=disk)
        later = [pipe.process(blk[i]) for i in stuck]
        check("A1.block-then-room", full_at == 200 and bool(stuck) and all(later) and exactly_once(archive_lines_sim(disk, "/sim/archive"), 393)
              and pipe.export()["projections"] == self.e0["projections"], {"archived_when_full": full_at, "unacknowledged": len(stuck)})
        pipe, disk = self.sim_run(lines, plan_key="archive_drop")
        ex = pipe.export()
        check("A1.drop-keeps-metrics", ex["projections"] == self.e0["projections"] and ex["ledger"]["archive_dropped"] == 193
              and exactly_once(archive_lines_sim(disk, "/sim/archive"), 200), ex["ledger"])
        pipe, _, _ = deliver(Factory("/sim", self.registry, self.plan_text["main"], disk=disk), batches_of(lines, 10))
        ex = pipe.export()
        check("A1.drop-is-final", ex["projections"] == self.e0["projections"] and ex["ledger"]["archive_dropped"] == 193
              and exactly_once(archive_lines_sim(disk, "/sim/archive"), 200), ex["ledger"])
        hashed = os.path.join(self.out, "state-keyhashed", "archive")
        iso = P(os.path.join(self.out, "state-isolated-privacy"), self.registry, self.plan_text["main"], archive_override=False)
        try:
            iso.replay(hashed)
            refused = None
        except ArchiveRefused as e:
            refused = e.what
        check("A1.replay-under-other-privacy-refused", refused == "Session.1.session", refused)

    # -- P4: adversarial validation, and S1's equations with quarantine
    def adversarial(self):
        lines, probe = self.lines, self.probe
        good = json.loads(lines[0])

        def craft(seq, base=None, **over):
            r = json.loads(json.dumps(base or good))
            r["id"] = {"producer": "fe" * 16, "seq": seq}
            for k, v in over.items():
                if k == "fields":
                    r["fields"].update(v)
                else:
                    r[k] = v
            return json.dumps(r, sort_keys=True)

        audit = next(json.loads(x) for x in lines if '"schema": "Audit"' in x)
        dup = craft(15)
        dup = dup.replace('"fields": {', '"fields": {"region": "%s"}, "fields": {' % probe, 1)       # a second `fields`
        bad = ['{"not json ' + probe, dup,
               json.dumps({"fields": {}, "id": {"seq": 1}, "kind": "observation-event", "schema": probe, "truncated": [], "version": 1, "withheld": 0}),
               craft(1, schema=probe), craft(2, version=7, fields={"region": probe})]
        undeclared = json.loads(craft(3))
        undeclared["fields"][probe] = "x"
        bad += [json.dumps(undeclared, sort_keys=True), craft(4, fields={"region": probe}), craft(5, fields={"email": probe}), craft(6, fields={"note": probe}),
                craft(7, fields={"card": probe}), craft(8, withheld=1, fields={"amount": probe}), craft(9, truncated=[probe]),
                craft(10, base=audit, fields={"order": "1.5"}), craft(11, fields={"amount": "99999999999999999999"}), craft(12, fields={"sku": "x" * 9}),
                craft(13, truncated=["email"]), craft(14, fields={"amount": "1" * 5000})]
        want_q = {"unknown-schema": 1, "unknown-version": 1, "undeclared-field": 2, "ill-typed": 9, "withheld": 1}
        st = os.path.join(self.out, "state-adversarial")
        pipe, _, _ = deliver(Factory(st, self.registry, self.plan_text["main"]), batches_of(lines[:200] + bad + lines[200:] + bad, 10))
        ex = pipe.export()
        open(os.path.join(self.out, "export-adversarial.json"), "w").write(json.dumps(ex, sort_keys=True))
        check("P4.quarantine-codes", ex["ledger"]["quarantine"] == want_q and ex["ledger"]["malformed_deliveries"] == 6 and ex["projections"] == self.e0["projections"], ex["ledger"])
        check("S1.equations-with-quarantine", ex["archive"]["records"] + sum(want_q.values()) == 393 + len(bad) - 3 and ex["ledger"]["accepted"] == 393)
        self.bad_lines = bad

    # -- L1: the plan corpus, the YAML subset, and accepted predicates by value
    def validation(self):
        wrong = []
        for name, text in sorted(self.plan_text.items()):
            marks = [(m.group(1), i + 1) for i, ln in enumerate(text.splitlines()) for m in [re.search(r"# expect: (E-TEL-\d+)", ln)] if m]
            try:
                planmod.load(text, self.registry)
                got = []
            except planmod.PlanInvalid as e:
                got = [(d.code, d.line) for d in e.diags]
            if got != marks:
                wrong.append({"plan": name, "want": marks, "got": got})
        check("L1.codes-at-lines", not wrong and sum(1 for n in self.plan_text if n.startswith("bad_")) >= 20, wrong)
        cases = {'a: "x\\"#y"  # c': {"a": 'x"#y'}, 'a: "p\\nq"': {"a": "p\nq"}, "a: 'it''s' # c": {"a": "it's"}, 'a: ["p,q", r]': {"a": ["p,q", "r"]},
                 'a: b # c': {"a": "b"}, 'a: "\\u00e9"': {"a": "é"}}
        cases['"telemetr\\u0079": 1'] = {"telemetry": 1}
        cases["a: 9007199254740993.0"] = {"a": Decimal("9007199254740993.0")}
        yaml_ok = all(yamlite.loads(t).plain() == v for t, v in cases.items())
        refused = True
        for bad_text in ('a: "\\x41"', 'a: "\\uD800"'):
            try:
                yamlite.loads(bad_text)
                refused = False
            except yamlite.PlanSyntax:
                pass
        check("L1.yaml-quoted-scalars", yaml_ok and refused)
        pipe, _ = self.sim_run(self.lines, plan_key="predicates")
        compare("L1.predicates-select-by-value", pipe.export(), "predicates", self.lines, self.F["P2"])

    # -- P1: the egress scan over every byte written (streams, channels, states, exports, simulated disks, this log)
    def egress(self):
        disk = SimDisk(crash_at=self.total_ops // 2)
        deliver(Factory("/sim", self.registry, self.plan_text["main"], disk=disk), batches_of(self.lines + getattr(self, "bad_lines", []), 25))
        for n, data in sim_files(disk).items():
            open(os.path.join(self.out, "simdisk" + n.replace("/", "_")), "wb").write(data)
        # every byte that run wrote, including what a crash or a later write took away
        open(os.path.join(self.out, "simdisk-written.log"), "wb").write(b"\n".join(disk.written))
        hits, scanned = [], 0
        pats = {c: canary_patterns(c) for c in self.all_cores}
        for base, _, files in os.walk(self.out):
            for fn in files:
                data = open(os.path.join(base, fn), "rb").read()
                scanned += len(data)
                for c, ps in pats.items():
                    for pat in ps:
                        if pat.encode() in data:
                            hits.append({"file": os.path.relpath(os.path.join(base, fn), self.out), "canary": c[:3], "pattern_length": len(pat)})
                            break
        streams = len(os.listdir(os.path.join(self.out, "streams")))
        check("P1.no-canary-in-any-output", not hits and scanned > 1_000_000 and streams == 2 * self.backend.n, {"bytes_scanned": scanned, "streams": streams, "hits": hits[:10]})


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
