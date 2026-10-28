"""The pipeline process (SP-09 design 3.2 to 3.8): validation and the ledger, the archive, projection epochs,
acknowledgement, recovery, replay and export. Records arrive as canonical permitted records written by the
producers' adapters; nothing here ever holds a raw value of a field the policy transforms, and nothing here writes a
record's content anywhere but the archive.

Each consumer (the archive, each active projection epoch, the ledger) keeps its own applied set and commits it with
its own state; a batch is acknowledged only when all of them have committed it."""
from __future__ import annotations

import hashlib
import json
import os
import re
from decimal import Decimal, InvalidOperation
from typing import Dict, List, Optional, Set, Tuple

from telemetry import plan as planmod
from telemetry.disk import RealDisk
from telemetry.registry import FieldInfo, Registry

PRODUCER = re.compile(r"[0-9a-f]{32}")
HASHED = re.compile(r"h1:[A-Za-z0-9_.-]+:[0-9a-f]{32}")
INTEGER = re.compile(r"-?[0-9]+")
DECIMAL = re.compile(r"-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?")
BOUNDS = {"Int8": (-2**7, 2**7 - 1), "Int16": (-2**15, 2**15 - 1), "Int32": (-2**31, 2**31 - 1), "Int64": (-2**63, 2**63 - 1), "Int": (-2**63, 2**63 - 1),
          "UInt8": (0, 2**8 - 1), "UInt16": (0, 2**16 - 1), "UInt32": (0, 2**32 - 1), "UInt64": (0, 2**64 - 1), "UInt": (0, 2**64 - 1), "Byte": (0, 255)}
FRAME_KEYS = {"fields", "function", "id", "kind", "outcome", "schema", "truncated", "version", "withheld"}
EVENT_KEYS = FRAME_KEYS - {"function", "outcome"}
OVERFLOW = "__overflow__"


class ArchiveRefused(Exception):
    """A replay or recovery that cannot use an archive: a closed segment that does not match its manifest digest, or
    an archive whose records were transformed under privacy actions other than the plan's."""

    def __init__(self, what: str, why: str):
        super().__init__(f"archive {what}: {why}")
        self.what = what
        self.segment = what


ArchiveCorrupt = ArchiveRefused


class IdSet:
    """Per producer: the highest sequence w with every sequence 1..w applied, and the applied sequences above w."""

    def __init__(self):
        self.w: Dict[str, int] = {}
        self.above: Dict[str, Set[int]] = {}

    def has(self, p: str, s: int) -> bool:
        return s <= self.w.get(p, 0) or s in self.above.get(p, ())

    def add(self, p: str, s: int):
        if self.has(p, s):
            return
        w = self.w.get(p, 0)
        if s == w + 1:
            up = self.above.get(p, set())
            w = s
            while w + 1 in up:
                w += 1
                up.discard(w)
            self.w[p] = w
        else:
            self.above.setdefault(p, set()).add(s)

    def count(self) -> int:
        return sum(self.w.values()) + sum(len(a) for a in self.above.values())

    def covers(self, other: "IdSet") -> bool:
        return all(self.has(p, s) for p, s in other.items())

    def items(self):
        for p in set(self.w) | set(self.above):
            yield from ((p, s) for s in range(1, self.w.get(p, 0) + 1))
            yield from ((p, s) for s in self.above.get(p, ()))

    def to_json(self):
        return {p: [self.w.get(p, 0), sorted(self.above.get(p, ()))] for p in sorted(set(self.w) | set(self.above))}

    @staticmethod
    def from_json(d) -> "IdSet":
        s = IdSet()
        for p, (w, up) in d.items():
            if w:
                s.w[p] = w
            if up:
                s.above[p] = set(up)
        return s


def _atomic(disk, path: str, obj):
    """Write a state file so that a crash leaves the old or the new one: temporary file, fsync, rename, fsync dir."""
    tmp = path + ".tmp"
    disk.write_new(tmp, (json.dumps(obj, sort_keys=True) + "\n").encode())
    disk.fsync(tmp)
    disk.rename(tmp, path)
    disk.fsync_dir(os.path.dirname(path))


def _load(disk, path: str):
    b = disk.read(path)
    return None if b is None else json.loads(b)


def _settle(disk, path: str):
    """Make a file found on recovery durable, with its directory entry, before anything is acknowledged: a process
    killed between a write's fsync and its directory's (or between a rename and its directory's) leaves a file the
    restarted pipeline reads but a power loss would still take away."""
    if disk.read(path) is not None:
        disk.fsync(path)
        disk.fsync_dir(os.path.dirname(path))


def _mkdir(disk, d: str):
    """A directory whose entry is durable before anything is written in it, as is every ancestor's: each is created
    if missing and its parent synced in any case, since an existing entry may have been left by a process killed
    before its parent's sync."""
    d = d.rstrip("/")
    parent = os.path.dirname(d)
    if not d or parent == d:
        return
    _mkdir(disk, parent)
    if not disk.is_dir(d):
        disk.makedirs(d)
    disk.fsync_dir(parent or "/")


def _unique(pairs):
    """An object with every key once: a second `fields` would be validated in place of the first while the raw bytes
    kept both, so a duplicate key at any depth makes the record malformed."""
    keys = [k for k, _ in pairs]
    if len(keys) != len(set(keys)):
        raise ValueError("duplicate key")
    return dict(pairs)


def validate(line: bytes, registry: Registry, plan: planmod.Plan):
    """(record, None) or (identity or None, code): design 3.3. Nothing of the line's content leaves this function in
    a rejection."""
    try:
        rec = json.loads(line, object_pairs_hook=_unique)
    except (ValueError, UnicodeDecodeError, RecursionError):
        return None, "malformed"
    if not isinstance(rec, dict):
        return None, "malformed"
    ident = rec.get("id")
    if not (isinstance(ident, dict) and set(ident) == {"producer", "seq"} and isinstance(ident["producer"], str) and PRODUCER.fullmatch(ident["producer"])
            and isinstance(ident["seq"], int) and not isinstance(ident["seq"], bool) and ident["seq"] >= 1):
        return None, "malformed"
    who = (ident["producer"], ident["seq"])
    kind = rec.get("kind")
    if kind not in ("observation-frame", "observation-event") or set(rec) != (FRAME_KEYS if kind == "observation-frame" else EVENT_KEYS):
        return who, "malformed"
    if not (isinstance(rec["schema"], str) and isinstance(rec["version"], int) and isinstance(rec["withheld"], int) and isinstance(rec["fields"], dict)
            and isinstance(rec["truncated"], list) and all(isinstance(t, str) for t in rec["truncated"])):
        return who, "malformed"
    if kind == "observation-frame" and not (isinstance(rec["function"], str) and rec["outcome"] in planmod.OUTCOMES):
        return who, "malformed"
    if registry.latest(rec["schema"]) is None:
        return who, "unknown-schema"
    declared = registry.fields(rec["schema"], rec["version"])
    if declared is None:
        return who, "unknown-version"
    if rec["withheld"] != 0:
        return who, "withheld"
    for f, v in rec["fields"].items():
        if f not in declared:
            return who, "undeclared-field"
        if not isinstance(v, str) or not _typed(v, declared[f], plan.action(rec["schema"], f, rec["version"])):
            return who, "ill-typed"
    for t in rec["truncated"]:
        if t not in declared:
            return who, "undeclared-field"
        if plan.action(rec["schema"], t, rec["version"]) != "permit":
            return who, "ill-typed"                        # a transformed field's cut is never told (design 3.1.4)
    return rec, None


def _typed(v: str, info: FieldInfo, action: str) -> bool:
    """Whether `v` is a value of the field's post-transform type (design 3.6): the declared type, its integer range and
    its scalar bound, for a permitted field."""
    if action == "redact":
        return v == "[redacted]"
    if action == "hash":
        return HASHED.fullmatch(v) is not None
    if action == "drop":
        return False
    kind = info.kind()
    if kind == "enum":
        return v in info.variants
    if kind == "numeric":
        b = info.base()
        if b in BOUNDS:
            # no 64-bit value has more than 20 digits: refuse longer text before converting it
            return len(v) <= 21 and INTEGER.fullmatch(v) is not None and BOUNDS[b][0] <= int(v) <= BOUNDS[b][1]
        try:
            Decimal(v)
        except InvalidOperation:
            return False
        return DECIMAL.fullmatch(v) is not None
    if kind == "bool":
        return v in ("true", "false")
    if kind == "string":
        m = re.search(r"scalars:([0-9]+)\.\.([0-9]+)", info.type)
        return m is None or int(m.group(1)) <= len(v) <= int(m.group(2))
    return True


def identity_of(rec: dict) -> Tuple[str, int]:
    return rec["id"]["producer"], rec["id"]["seq"]


def privacy_actions(plan: planmod.Plan, registry: Registry) -> Dict[str, str]:
    """The action for every registered (schema, version, field), keyed `Schema|version|field`."""
    return {f"{s}|{v}|{f}": plan.action(s, f, v) for s in registry.schemas for v in registry.versions(s) for f in registry.fields(s, v)}


class Archive:
    """JSON Lines segments and an atomically published manifest (design 3.4); `privacy.json` beside them names the
    actions its records were transformed under."""

    def __init__(self, disk, d: str, segment_records: int, capacity: Optional[int]):
        self.disk, self.d, self.segment_records, self.capacity = disk, d, segment_records, capacity
        _mkdir(disk, d)
        self.applied = IdSet()
        self.digest: Dict[Tuple[str, int], str] = {}
        self.closed: List[dict] = []
        self.open_name = "seg-000001.jsonl"
        self.open_count = 0
        self.pending: List[bytes] = []
        self.recover()

    def path(self, name: str) -> str:
        return os.path.join(self.d, name)

    def recover(self):
        for name in ("manifest.json", "privacy.json"):
            _settle(self.disk, self.path(name))
        m = _load(self.disk, self.path("manifest.json")) or {"closed": [], "open": "seg-000001.jsonl"}
        self.closed, self.open_name = m["closed"], m["open"]
        for seg in self.closed:
            _settle(self.disk, self.path(seg["name"]))
            data = self.disk.read(self.path(seg["name"]))
            if data is None or hashlib.sha256(data).hexdigest() != seg["sha256"]:
                raise ArchiveRefused(seg["name"], "does not match its manifest sha256")
            for ln in data.splitlines():
                self._index(ln)
        _settle(self.disk, self.path(self.open_name))
        data = self.disk.read(self.path(self.open_name))
        self.open_count = 0
        if data is not None:
            good = 0
            for ln in data.split(b"\n")[:-1]:
                try:
                    json.loads(ln)
                except ValueError:
                    break
                good += len(ln) + 1
                self._index(ln)
                self.open_count += 1
            if good != len(data):
                # a torn final write: cut back to the last complete record
                self.disk.truncate(self.path(self.open_name), good)
                self.disk.fsync(self.path(self.open_name))

    def _index(self, ln: bytes):
        rec = json.loads(ln)
        who = identity_of(rec)
        self.applied.add(*who)
        self.digest[who] = hashlib.sha256(ln).hexdigest()

    def records(self) -> int:
        return self.applied.count()

    def admit(self, rec: dict, line: bytes, dropped: IdSet) -> str:
        """`new`, `dup`, `conflict`, `dropped` (a drop is final for its identity) or `full`."""
        who = identity_of(rec)
        if self.applied.has(*who):
            return "dup" if self.digest.get(who) == hashlib.sha256(line).hexdigest() else "conflict"
        if dropped.has(*who):
            return "dropped"
        if self.capacity is not None and self.records() >= self.capacity:
            return "full"
        self.applied.add(*who)
        self.digest[who] = hashlib.sha256(line).hexdigest()
        self.pending.append(line)
        return "new"

    def commit(self):
        while self.pending:
            if self.open_count >= self.segment_records:
                self._close()
            room = self.segment_records - self.open_count
            chunk, self.pending = self.pending[:room], self.pending[room:]
            path = self.path(self.open_name)
            fresh = self.disk.read(path) is None
            self.disk.append(path, b"".join(ln + b"\n" for ln in chunk))
            self.disk.fsync(path)
            if fresh:
                self.disk.fsync_dir(self.d)             # the new segment's directory entry is durable too
            self.open_count += len(chunk)
        if self.open_count >= self.segment_records:
            self._close()

    def _close(self):
        data = self.disk.read(self.path(self.open_name))
        self.closed = self.closed + [{"name": self.open_name, "sha256": hashlib.sha256(data).hexdigest(), "records": self.open_count}]
        n = int(self.open_name[4:10]) + 1
        self.open_name, self.open_count = f"seg-{n:06d}.jsonl", 0
        _atomic(self.disk, self.path("manifest.json"), {"closed": self.closed, "open": self.open_name})


def read_archive(disk, d: str, actions: Optional[Dict[str, str]] = None) -> List[bytes]:
    """Every archived record in manifest order (design 3.7), after checking every closed segment and, given the plan's
    actions, that the archive's records were transformed under the same ones. Any mismatch refuses the whole replay,
    naming what, before anything is applied."""
    if actions is not None:
        theirs = _load(disk, os.path.join(d, "privacy.json"))
        if theirs is None:
            raise ArchiveRefused("privacy.json", "absent: the actions its records were transformed under are unknown")
        for k in sorted(set(theirs) & set(actions)):
            if theirs[k] != actions[k]:
                raise ArchiveRefused(k.replace("|", "."), f"was {theirs[k]} when archived and is {actions[k]} in the plan")
    m = _load(disk, os.path.join(d, "manifest.json")) or {"closed": [], "open": "seg-000001.jsonl"}
    out: List[bytes] = []
    for seg in m["closed"]:
        data = disk.read(os.path.join(d, seg["name"]))
        if data is None or hashlib.sha256(data).hexdigest() != seg["sha256"]:
            raise ArchiveRefused(seg["name"], "does not match its manifest sha256")
        out += data.splitlines()
    data = disk.read(os.path.join(d, m["open"])) or b""
    for ln in data.split(b"\n")[:-1]:
        try:
            json.loads(ln)
        except ValueError:
            break
        out.append(ln)
    return out


class Epoch:
    """One projection epoch's state (design 3.4 to 3.6), committed atomically."""

    def __init__(self, disk, path: str, pr: planmod.Projection, registry: Registry):
        self.disk, self.path, self.pr, self.registry = disk, path, pr, registry
        _settle(disk, path)
        s = _load(disk, path)
        if s is None:
            s = {"applied": {}, "series": [], "values": {m.name: {} for m in pr.metrics}, "lifecycles": {},
                 "anomalies": {m.name: {} for m in pr.metrics if m.instrument == "updown"}, "missing": {m.name: 0 for m in pr.metrics if m.instrument == "histogram"},
                 "admitted": 0, "overflow_records": 0}
        self.applied = IdSet.from_json(s["applied"])
        self.series: List[str] = s["series"]
        self.allocated = set(self.series)
        self.s = s
        self.dirty = False
        self.per_record = any(m.instrument != "updown" for m in pr.metrics)

    def kinds(self, version: int) -> Dict[str, str]:
        return {f: i.kind() for f, i in self.registry.fields(self.pr.schema, version).items()}

    def alloc(self, key: str) -> str:
        if key in self.allocated:
            return key
        if len(self.series) < self.pr.limit:
            self.series.append(key)
            self.allocated.add(key)
            return key
        return OVERFLOW

    def apply(self, rec: dict):
        who = identity_of(rec)
        if self.applied.has(*who):
            return
        self.applied.add(*who)
        self.dirty = True
        if rec["schema"] != self.pr.schema or rec["version"] not in self.pr.admitted:
            return
        kinds = self.kinds(rec["version"])
        member = self.pr.where is None or self.pr.where.holds(rec, kinds)
        key = json.dumps([rec.get("outcome") if d == "outcome" else rec["fields"].get(d) for d in self.pr.dimensions])
        slot = None
        if member:
            self.s["admitted"] += 1
            if self.per_record:
                # every admitted record takes its tuple's series, in the order records are applied (design 3.6)
                slot = self.alloc(key)
                if slot == OVERFLOW:
                    self.s["overflow_records"] += 1
        for m in self.pr.metrics:
            vals = self.s["values"][m.name]
            if m.instrument == "counter" and member:
                vals[slot] = vals.get(slot, 0) + 1
            elif m.instrument == "histogram" and member:
                v = rec["fields"].get(m.value)
                if v is None:
                    self.s["missing"][m.name] += 1
                    continue
                h = vals.setdefault(slot, {"count": 0, "sum": "0", "buckets": [0] * (len(m.buckets) + 1)})
                d = Decimal(v)
                h["count"] += 1
                h["sum"] = str(Decimal(h["sum"]) + d)
                h["buckets"][next((i for i, b in enumerate(m.buckets) if d <= b), len(m.buckets))] += 1
            elif m.instrument == "updown":
                slot = self.lifecycle(m, rec, kinds, member, key, vals, slot)

    def lifecycle(self, m: planmod.Metric, rec: dict, kinds, member: bool, key: str, vals: dict, slot):
        """One up-down metric's transition; returns the record's series, allocated at most once per record."""
        is_start, is_end = m.start.holds(rec, kinds), m.end.holds(rec, kinds)
        if not (is_start or is_end):
            return slot
        an = self.s["anomalies"][m.name]
        k = rec["fields"].get(m.key)
        if k is None or (is_start and is_end):
            name = "missing_key" if k is None else "start_and_end"
            an[name] = an.get(name, 0) + 1
            return slot
        # a lifecycle is one metric's, scoped to the producer incarnation that wrote it (design 3.5)
        lk = m.name + "|" + rec["id"]["producer"] + "|" + k
        st = self.s["lifecycles"].get(lk)
        if is_start:
            if st is None or st[0] == "ended-first":
                bound = None
                if member:
                    if slot is None:
                        slot = self.alloc(key)
                        if slot == OVERFLOW:
                            self.s["overflow_records"] += 1
                    bound = slot
                    vals.setdefault(bound, 0)              # an allocated series exists, at zero, from its first start
                if st is None:
                    if bound is not None:
                        vals[bound] += 1
                    self.s["lifecycles"][lk] = ["open", bound]
                else:
                    self.s["lifecycles"][lk] = ["closed", bound]   # its end came first: bound, never incremented
            else:
                name = "duplicate_start" if st[0] == "open" else "reuse"
                an[name] = an.get(name, 0) + 1
        else:
            if st is None:
                self.s["lifecycles"][lk] = ["ended-first", None]
            elif st[0] == "open":
                if st[1] is not None:
                    vals[st[1]] -= 1
                self.s["lifecycles"][lk] = ["closed", st[1]]
            else:
                an["duplicate_end"] = an.get("duplicate_end", 0) + 1
        return slot

    def commit(self):
        if self.dirty:
            self.s["applied"] = self.applied.to_json()
            self.s["series"] = self.series
            _atomic(self.disk, self.path, self.s)
            self.dirty = False

    def export(self) -> dict:
        ms = {}
        for m in self.pr.metrics:
            vals = self.s["values"][m.name]
            series = [[json.loads(k), vals[k]] for k in sorted(vals) if k != OVERFLOW]
            ms[m.name] = {"instrument": m.instrument, "unit": m.unit, "series": series, "overflow": vals.get(OVERFLOW)}
        states: Dict[str, int] = {}
        for st, _ in self.s["lifecycles"].values():
            states[st] = states.get(st, 0) + 1
        return {"epoch": self.pr.epoch, "dimensions": self.pr.dimensions, "metrics": ms, "anomalies": self.s["anomalies"],
                "missing": self.s["missing"], "admitted": self.s["admitted"], "overflow_records": self.s["overflow_records"],
                "allocated": [json.loads(k) for k in sorted(self.series)], "lifecycle_states": states, "export": self.pr.export}


class Pipeline:
    def __init__(self, state: str, registry: Registry, plan_text: str, disk=None, archive_override: Optional[bool] = None):
        self.disk = disk if disk is not None else RealDisk()
        self.state, self.registry = state, registry
        self.plan = planmod.load(plan_text, registry)
        if archive_override is not None:
            self.plan.archive["enabled"] = archive_override
        _mkdir(self.disk, state)                           # the state directory itself, though it may exist already
        _mkdir(self.disk, os.path.join(state, "epochs"))
        for name in ("ledger.json", "plans.json", "privacy.json"):
            _settle(self.disk, os.path.join(state, name))
        self.ledger_path = os.path.join(state, "ledger.json")
        self.ledger = _load(self.disk, self.ledger_path) or {"quarantine": {}, "malformed_deliveries": 0, "archive_dropped": {}, "conflicts": {},
                                                               "delivered": 0, "accepted": {}, "seen": {}}
        self.ledger_seen = IdSet.from_json(self.ledger["seen"])
        self.dropped = IdSet.from_json(self.ledger["archive_dropped"])
        self.accepted = IdSet.from_json(self.ledger["accepted"])
        self.history_path = os.path.join(state, "plans.json")
        self.history = _load(self.disk, self.history_path) or {"epochs": {}, "active": {}}
        # every constraint against the state's history is checked before anything is written: a rejected plan
        # leaves the state as it was
        privacy, privacy_diags = self.check_privacy()
        diags = privacy_diags + self.check_epochs()
        if diags:
            raise planmod.PlanInvalid(diags)
        self.actions = privacy
        if privacy != (_load(self.disk, os.path.join(state, "privacy.json")) or {}):
            _atomic(self.disk, os.path.join(state, "privacy.json"), privacy)
        a = self.plan.archive
        self.archive = Archive(self.disk, os.path.join(state, "archive"), a["segment_records"], a["capacity_records"]) if a["enabled"] else None
        if self.archive is not None and _load(self.disk, os.path.join(self.archive.d, "privacy.json")) != self.actions:
            _atomic(self.disk, os.path.join(self.archive.d, "privacy.json"), self.actions)
        self.epochs: Dict[str, Epoch] = {}
        self.activate()

    def check_privacy(self):
        """Privacy actions are fixed per schema version for the life of this state (design 3.7): a plan that changes
        one is E-TEL-007 at its line; a newly registered version adds its actions. Returns the merged actions and the
        diagnostics, writing nothing."""
        had = _load(self.disk, os.path.join(self.state, "privacy.json")) or {}
        now = privacy_actions(self.plan, self.registry)
        diags = []
        for k in sorted(set(had) & set(now)):
            if had[k] != now[k]:
                s, v, f = k.split("|")
                diags.append(planmod.Diag("E-TEL-007", self.plan.privacy_line(s, f, int(v)),
                                          f"`{s}.v{v}.{f}` was {had[k]} when its records were taken and the plan makes it {now[k]}; privacy is fixed per schema version, so this needs a new version"))
        return dict(had, **now), diags

    def check_epochs(self):
        """E-TEL-006: a new epoch may not use a metric name of any other epoch in the history (Platform 6.4)."""
        used = {name: (proj, ep) for proj, eps in self.history["epochs"].items() for ep, info in eps.items() for name in info["metrics"]}
        diags = []
        for name, pr in self.plan.projections.items():
            if pr.epoch in self.history["epochs"].get(name, {}):
                continue
            for m in pr.metrics:
                if m.name in used and used[m.name] != (name, pr.epoch):
                    proj, ep = used[m.name]
                    diags.append(planmod.Diag("E-TEL-006", m.line, f"metric `{m.name}` belongs to epoch {ep} of `{proj}`; an incompatible change needs a new metric name (Platform 6.4)"))
        return diags

    def activate(self):
        """Make the plan's epochs active, catching up a new or reactivated epoch from the archive (design 3.7)."""
        changed = False
        for name, pr in sorted(self.plan.projections.items()):
            d = os.path.join(self.state, "epochs", name)
            _mkdir(self.disk, d)
            ep = Epoch(self.disk, os.path.join(d, pr.epoch + ".json"), pr, self.registry)
            self.epochs[name] = ep
            # every start drains, in archive order, the archived records the epoch lacks (a new or reactivated epoch's
            # catch-up; after a crash, a batch the archive committed and the epoch did not), so a redelivery, however
            # it is batched or ordered, finds them applied in archive order
            if self.archive is not None:
                for ln in read_archive(self.disk, self.archive.d):
                    ep.apply(json.loads(ln))
                ep.commit()
            if self.history["active"].get(name) != pr.epoch:
                info = self.history["epochs"].setdefault(name, {}).setdefault(pr.epoch, {"metrics": [m.name for m in pr.metrics]})
                # complete only when every record ever accepted reached this epoch: none yet, or all in the archive
                if self.accepted.count() == 0:
                    info["coverage"] = "complete"
                elif self.archive is not None:
                    info["coverage"] = "complete" if self.archive.applied.covers(self.accepted) else "archive_partial"
                else:
                    info["coverage"] = "since_activation"
                self.history["active"][name] = pr.epoch
                changed = True
        for name in list(self.history["active"]):
            if name not in self.plan.projections:
                del self.history["active"][name]          # retired: its state is kept and applies nothing
                changed = True
        if changed:
            _atomic(self.disk, self.history_path, self.history)

    def process(self, lines: List[bytes]) -> bool:
        """Apply one delivered batch; True when it may be acknowledged (every consumer has committed it)."""
        blocked = False
        valid: List[dict] = []
        for ln in lines:
            rec, code = validate(ln, self.registry, self.plan)
            self.ledger["delivered"] += 1
            if code is not None:
                who = rec
                if who is None:
                    self.ledger["malformed_deliveries"] += 1     # no identity to deduplicate by: counted per delivery
                elif not self.ledger_seen.has(*who):
                    self.ledger_seen.add(*who)
                    self.ledger["quarantine"][code] = self.ledger["quarantine"].get(code, 0) + 1
                continue
            who = identity_of(rec)
            canonical = json.dumps(rec, sort_keys=True).encode()   # only what was validated is ever kept
            if self.archive is not None:
                st = self.archive.admit(rec, canonical, self.dropped)
                if st == "conflict":
                    k = f"{who[0]}|{who[1]}"
                    if k not in self.ledger["conflicts"]:
                        self.ledger["conflicts"][k] = 1
                    continue                                   # the archived payload is kept; this one is not applied
                if st == "full":
                    if self.plan.archive["on_full"] == "block":
                        blocked = True
                    else:
                        self.dropped.add(*who)
            self.accepted.add(*who)
            valid.append(rec)
        # a record new to this batch takes its archive position in batch order, and an archived one is already in every
        # epoch (each start drains the archive into them), so epochs apply records in archive order (design 3.6)
        for rec in valid:
            for ep in self.epochs.values():
                ep.apply(rec)
        if self.archive is not None:
            self.archive.commit()
        for ep in self.epochs.values():
            ep.commit()
        self.ledger["archive_dropped"] = self.dropped.to_json()
        self.ledger["seen"] = self.ledger_seen.to_json()
        self.ledger["accepted"] = self.accepted.to_json()
        _atomic(self.disk, self.ledger_path, self.ledger)
        return not blocked

    def replay(self, archive_dir: str):
        """Backfill this destination's epochs from an archive whose records were transformed under the plan's privacy
        actions; their applied sets prevent double counting."""
        for ln in read_archive(self.disk, archive_dir, self.actions):
            rec = json.loads(ln)
            self.accepted.add(*identity_of(rec))            # what reached this destination, for later coverage
            for ep in self.epochs.values():
                ep.apply(rec)
        for ep in self.epochs.values():
            ep.commit()
        self.ledger["accepted"] = self.accepted.to_json()
        _atomic(self.disk, self.ledger_path, self.ledger)

    def export(self) -> dict:
        out = {"projections": {}, "ledger": {"quarantine": self.ledger["quarantine"], "malformed_deliveries": self.ledger["malformed_deliveries"],
                                             "archive_dropped": self.dropped.count(), "identity_conflicts": len(self.ledger["conflicts"]),
                                             "accepted": self.accepted.count()},
               "archive": None if self.archive is None else {"records": self.archive.records(), "closed_segments": len(self.archive.closed)}}
        for name, ep in sorted(self.epochs.items()):
            e = ep.export()
            e["coverage"] = self.history["epochs"][name][ep.pr.epoch]["coverage"]
            out["projections"][name] = e
        return out


def export_text(export: dict) -> str:
    return json.dumps(export, sort_keys=True) + "\n"
